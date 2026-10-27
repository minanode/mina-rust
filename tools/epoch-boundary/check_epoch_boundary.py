#!/usr/bin/env python3
"""Check a mina-rust node's view of an epoch boundary against OCaml.

Read-only: it queries the node's HTTP and GraphQL endpoints, public OCaml
GraphQL nodes and the public precomputed block bucket. It starts nothing.

Live mode (default), run against a synced node:

    check_epoch_boundary.py --node http://localhost:3200 --out /tmp/epoch7

1. The node is synced and its best chain (root to tip, about 290 blocks)
   follows OCaml's epoch rules block by block (`update_pair` in
   `proof_of_stake.ml`): the staking data carry over within an epoch, and on
   the first block of a new epoch the staking data become the old next epoch
   data, the next epoch ledger becomes the parent's snarked ledger, the
   start checkpoint the parent's hash, and so on.
2. If the boundary is in the node's frontier, the pair of blocks across it
   (P, the last block of the old epoch, and T, the first of the new one) is
   printed, and checked against the same rules.
3. The node's tip, P and T are compared field by field with OCaml: the
   GraphQL `block(stateHash:)` of each OCaml node given, and the precomputed
   block that OCaml's archive uploader wrote to the public bucket.
4. The node's tip is compared with the OCaml nodes' best tips.
5. The node holds the new staking ledger: the stake winner of T (or of the
   tip) must be among the delegators of T's creator in the node's staking
   ledger (GraphQL `account { delegators }` reads that ledger by hash). The
   blockchain proof shows the winner's account is in the staking ledger
   with the creator as delegate. Skip with `--no-ledger-probe`: it scans the
   whole ledger on the node's ledger thread (about 3 s).
6. Ledger resyncs and restarts recorded in `/stats/sync`.

Before the boundary it prints when the boundary and the end of the seed
update range are, and, once the seed is final, the staking data the next
epoch will have. `--save-prediction FILE` writes them; after the boundary,
`--prediction FILE` checks them.

Offline mode checks the same rules on a chain of precomputed blocks, such as
the replay data from `tools/block-replay/fetch_chain.py`:

    check_epoch_boundary.py --offline --blocks DIR --chain DIR/chain.txt

Exit status: 0 if every check passed, 1 if one failed, 2 if the script could
not run.
"""

import argparse
import concurrent.futures
import datetime
import hashlib
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

# Mesa mainnet. Genesis timestamp and protocol constants are those of
# `config_84b8d969.json`, the runtime config shipped in the 4.0.2 debs.
GENESIS_TIMESTAMP = 1788458400  # 2026-09-03T18:00:00Z
SLOT_SECONDS = 90
SLOTS_PER_EPOCH = 7140
K = 290
GENESIS_SLOT_SINCE_GENESIS = 958440
GENESIS_LEDGER_HASH = "jwo3EM3zXkK5gpQYZHQP9KJVgH4ZCfv5bMgvyy3Gh2dAWU6AgyN"
# Blocks per slot of the fastest and slowest whole epochs so far (epoch 0:
# 2964 blocks in 7140 slots; epoch 5: 2251), for time estimates.
PAST_EPOCH_RATES = (0.415, 0.315)

DEFAULT_OCAML_GRAPHQL = [
    # Auro's node runs 84b8d969 (4.0.2), Minascan's another build; both
    # report chain id 0718f61a... (checked 2026-10-19).
    "https://mina-mainnet-graphql.aurowallet.com/graphql",
    "https://api.minascan.io/node/mainnet/v1/graphql",
]
BUCKET = "mina_network_block_data"

EPOCH_FIELDS = (
    "ledger_hash",
    "ledger_total_currency",
    "seed",
    "start_checkpoint",
    "lock_checkpoint",
    "epoch_length",
)

BLOCK_FIELDS = """
    stateHash creator winnerAccount { publicKey }
    protocolState {
      previousStateHash
      blockchainState { snarkedLedgerHash stagedLedgerHash }
      consensusState {
        blockHeight epoch epochCount slot slotSinceGenesis totalCurrency
        stakingEpochData { %(epoch)s } nextEpochData { %(epoch)s }
      }
    }
""" % {
    # Inline: OCaml names the two types StakingEpochData and NextEpochData,
    # mina-rust has one EpochData, so a fragment fits only one of them.
    "epoch": "ledger { hash totalCurrency } seed startCheckpoint lockCheckpoint epochLength"
}

# --- base58check --------------------------------------------------------------

B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58check(payload):
    data = payload + hashlib.sha256(hashlib.sha256(payload).digest()).digest()[:4]
    n = int.from_bytes(data, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = B58[r] + out
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + out


# `State_hash.(of_hash zero)`: version byte 0x10, bin_prot version 1, then
# the field element. This is the lock checkpoint of a new epoch's first
# block when that block is outside the seed update range.
ZERO_STATE_HASH = b58check(bytes([0x10, 0x01]) + bytes(32))
assert ZERO_STATE_HASH == "3NK2tkzqqK5spR2sZ7tujjqPksL45M3UUrcA4WhCkeiPtnugyE2x"

# --- blocks -------------------------------------------------------------------


def epoch_data_from_graphql(d):
    return {
        "ledger_hash": d["ledger"]["hash"],
        "ledger_total_currency": int(d["ledger"]["totalCurrency"]),
        "seed": d["seed"],
        "start_checkpoint": d["startCheckpoint"],
        "lock_checkpoint": d["lockCheckpoint"],
        "epoch_length": int(d["epochLength"]),
    }


def epoch_data_from_precomputed(d):
    return {
        "ledger_hash": d["ledger"]["hash"],
        "ledger_total_currency": int(d["ledger"]["total_currency"]),
        "seed": d["seed"],
        "start_checkpoint": d["start_checkpoint"],
        "lock_checkpoint": d["lock_checkpoint"],
        "epoch_length": int(d["epoch_length"]),
    }


def block_from_graphql(b, source):
    ps = b["protocolState"]
    cs = ps["consensusState"]
    epoch, slot = int(cs["epoch"]), int(cs["slot"])
    return {
        "source": source,
        "state_hash": b["stateHash"],
        "prev_hash": ps["previousStateHash"],
        "height": int(cs["blockHeight"]),
        "epoch": epoch,
        "slot": slot,
        "slot_since_hard_fork": epoch * SLOTS_PER_EPOCH + slot,
        "slot_since_genesis": int(cs["slotSinceGenesis"]),
        "epoch_count": int(cs["epochCount"]),
        "total_currency": int(cs["totalCurrency"]),
        "snarked_ledger_hash": ps["blockchainState"]["snarkedLedgerHash"],
        "staged_ledger_hash": ps["blockchainState"]["stagedLedgerHash"],
        "staking": epoch_data_from_graphql(cs["stakingEpochData"]),
        "next": epoch_data_from_graphql(cs["nextEpochData"]),
        "creator": b.get("creator"),
        "winner": (b.get("winnerAccount") or {}).get("publicKey"),
    }


def block_from_protocol_state(ps, state_hash, source):
    """A block from a precomputed block's `protocol_state` (OCaml JSON)."""
    body = ps["body"]
    cs = body["consensus_state"]
    bs = body["blockchain_state"]
    gs = cs["curr_global_slot_since_hard_fork"]
    n, per_epoch = int(gs["slot_number"]), int(gs["slots_per_epoch"])
    return {
        "source": source,
        "state_hash": state_hash,
        "prev_hash": ps["previous_state_hash"],
        "height": int(cs["blockchain_length"]),
        "epoch": n // per_epoch,
        "slot": n % per_epoch,
        "slot_since_hard_fork": n,
        "slot_since_genesis": int(cs["global_slot_since_genesis"]),
        "epoch_count": int(cs["epoch_count"]),
        "total_currency": int(cs["total_currency"]),
        "snarked_ledger_hash": bs["ledger_proof_statement"]["target"]["first_pass_ledger"],
        "staged_ledger_hash": bs["staged_ledger_hash"]["non_snark"]["ledger_hash"],
        "staking": epoch_data_from_precomputed(cs["staking_epoch_data"]),
        "next": epoch_data_from_precomputed(cs["next_epoch_data"]),
        "creator": cs["block_creator"],
        "winner": cs["block_stake_winner"],
    }


COMPARED = (
    "state_hash",
    "prev_hash",
    "height",
    "epoch",
    "slot",
    "slot_since_genesis",
    "epoch_count",
    "total_currency",
    "snarked_ledger_hash",
    "staged_ledger_hash",
    "creator",
    "winner",
)


def compare(a, b):
    """Fields where two views of the same block differ."""
    diffs = []
    for f in COMPARED:
        if a.get(f) is not None and b.get(f) is not None and a[f] != b[f]:
            diffs.append(f"{f}: {a[f]} != {b[f]}")
    for which in ("staking", "next"):
        for f in EPOCH_FIELDS:
            if a[which][f] != b[which][f]:
                diffs.append(f"{which}.{f}: {a[which][f]} != {b[which][f]}")
    return diffs


def short(h):
    return f"{h[:8]}..{h[-4:]}"


# --- the epoch rules -----------------------------------------------------------


def check_pair(p, c, genesis_ledger_hash=GENESIS_LEDGER_HASH):
    """Check child `c` against parent `p` with OCaml's `update_pair`.

    Ports `Epoch_data.update_pair` and the part of `Consensus_state.update`
    that calls it; the block producer passes the parent's snarked ledger
    hash. Returns (kind, failures).

    OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L1062-L1109>
    OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L1855-L1923>
    """
    fails = []

    def expect(name, got, want):
        if got != want:
            fails.append(f"{name}: got {got}, want {want}")

    expect("prev_hash", c["prev_hash"], p["state_hash"])
    expect("height", c["height"], p["height"] + 1)
    in_seed_range = c["slot"] < SLOTS_PER_EPOCH * 2 // 3
    pn, cn = p["next"], c["next"]
    if c["epoch"] == p["epoch"]:
        kind = "same epoch"
        expect("epoch_count", c["epoch_count"], p["epoch_count"])
        for f in EPOCH_FIELDS:
            expect(f"staking.{f}", c["staking"][f], p["staking"][f])
        for f in ("ledger_hash", "ledger_total_currency", "start_checkpoint"):
            expect(f"next.{f}", cn[f], pn[f])
        expect("next.epoch_length", cn["epoch_length"], pn["epoch_length"] + 1)
    elif c["epoch"] > p["epoch"]:
        kind = "new epoch"
        expect("epoch_count", c["epoch_count"], p["epoch_count"] + 1)
        for f in EPOCH_FIELDS:
            expect(f"staking.{f} (= parent next.{f})", c["staking"][f], pn[f])
        if p["snarked_ledger_hash"] == genesis_ledger_hash:
            want_ledger = (pn["ledger_hash"], pn["ledger_total_currency"])
        else:
            want_ledger = (p["snarked_ledger_hash"], c["total_currency"])
        expect("next.ledger_hash (= parent snarked ledger)", cn["ledger_hash"], want_ledger[0])
        expect("next.ledger_total_currency", cn["ledger_total_currency"], want_ledger[1])
        expect("next.start_checkpoint (= parent hash)", cn["start_checkpoint"], p["state_hash"])
        expect("next.epoch_length", cn["epoch_length"], 1)
    else:
        return "epoch went back", [f"epoch {p['epoch']} -> {c['epoch']}"]
    # The seed and lock checkpoint move only inside the seed update range.
    if in_seed_range:
        expect("next.lock_checkpoint (= parent hash)", cn["lock_checkpoint"], p["state_hash"])
        if cn["seed"] == pn["seed"]:
            fails.append("next.seed did not change inside the seed update range")
    else:
        want_lock = ZERO_STATE_HASH if kind == "new epoch" else pn["lock_checkpoint"]
        expect("next.lock_checkpoint", cn["lock_checkpoint"], want_lock)
        expect("next.seed", cn["seed"], pn["seed"])
    return kind, fails


# --- reporting -----------------------------------------------------------------


class Report:
    def __init__(self):
        self.failed = 0
        self.passed = 0

    def check(self, name, ok, detail=""):
        tag = "PASS" if ok else "FAIL"
        if ok:
            self.passed += 1
        else:
            self.failed += 1
        print(f"[{tag}] {name}" + (f": {detail}" if detail else ""))
        return ok

    def info(self, name, detail=""):
        print(f"[INFO] {name}" + (f": {detail}" if detail else ""))

    def warn(self, name, detail=""):
        print(f"[WARN] {name}" + (f": {detail}" if detail else ""))


def utc(seconds):
    return datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc).strftime(
        "%Y-%m-%d %H:%M UTC"
    )


def utc_ns(nanos):
    if not nanos:
        return "never"
    return datetime.datetime.fromtimestamp(nanos / 1e9, datetime.timezone.utc).strftime(
        "%Y-%m-%d %H:%M:%S UTC"
    )


def slot_time(slot_since_hard_fork):
    return GENESIS_TIMESTAMP + slot_since_hard_fork * SLOT_SECONDS


def sync_stats_summary(stats):
    """Bootstraps, ledger fetches and resync labels from `/stats/sync`.

    Each snapshot repeats the ledgers and resync events of the one before it
    (`stats_sync.rs`, `new_target`), so they are deduplicated here: ledger
    fetches by (kind, hash, start time), resync labels by (kind, time).
    """
    stats = sorted(stats, key=lambda s: s["best_tip_received"])  # oldest first
    # A bootstrap whose target is replaced before it finishes leaves an
    # unsynced Bootstrap snapshot followed by another Bootstrap snapshot.
    bootstraps, current = [], None
    for s in stats:
        if s["kind"] != "Bootstrap":
            current = None
            continue
        if current is None:
            current = {"started": s["best_tip_received"], "synced": None, "retargets": 0}
            bootstraps.append(current)
        else:
            current["retargets"] += 1
        if s["synced"] is not None:
            current["synced"] = s["synced"]
            current = None
    last_synced = max((b["synced"] or 0 for b in bootstraps), default=0)
    fetches, seen = [], set()
    for s in stats:
        for kind, ledger in (s.get("ledgers") or {}).items():
            if not ledger:
                continue
            snarked, staged = ledger["snarked"], ledger["staged"]
            start = snarked.get("fetch_hashes_start") or staged.get("fetch_parts_start")
            key = (kind, snarked.get("hash"), start)
            if start is None or key in seen:
                continue
            seen.add(key)
            fetches.append(
                {
                    "kind": kind,
                    "hash": snarked.get("hash") or "",
                    "start": start,
                    "after_bootstrap": bool(last_synced) and start > last_synced,
                }
            )
    labels, seen = [], set()
    for s in stats:
        for r in s.get("resyncs") or []:
            kind = r["kind"] if isinstance(r["kind"], str) else json.dumps(r["kind"])
            if (kind, r["time"]) not in seen:
                seen.add((kind, r["time"]))
                labels.append({"kind": kind, "time": r["time"]})
    return {
        "snapshots": len(stats),
        "oldest": stats[0]["best_tip_received"] if stats else None,
        "bootstraps": bootstraps,
        "ledger_fetches": fetches,
        "resync_labels": labels,
    }


def print_block(label, b):
    print(f"  {label}: height {b['height']} {b['state_hash']}")
    print(
        f"     epoch {b['epoch']} slot {b['slot']} (since fork {b['slot_since_hard_fork']},"
        f" since genesis {b['slot_since_genesis']}), epoch_count {b['epoch_count']},"
        f" snarked ledger {b['snarked_ledger_hash']}"
    )
    for which in ("staking", "next"):
        d = b[which]
        print(
            f"     {which:7} ledger {d['ledger_hash']} ({d['ledger_total_currency']})"
            f" seed {d['seed']}"
        )
        print(
            f"             start {d['start_checkpoint']} lock {d['lock_checkpoint']}"
            f" length {d['epoch_length']}"
        )


# --- I/O -----------------------------------------------------------------------


def http(url, body=None, headers=None, timeout=60, attempts=3):
    # Some public endpoints answer 403 to urllib's default User-Agent.
    headers = {"user-agent": "mina-rust-epoch-check/1", **(headers or {})}
    if body is not None:
        headers["content-type"] = "application/json"
    for attempt in range(attempts):
        try:
            req = urllib.request.Request(url, data=body, headers=headers)
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.read()
        except urllib.error.HTTPError as e:
            if e.code == 404 or attempt == attempts - 1:
                raise
        except Exception:
            if attempt == attempts - 1:
                raise
        time.sleep(2)


def graphql(url, query):
    raw = http(url, json.dumps({"query": query}).encode())
    resp = json.loads(raw)
    if resp.get("errors"):
        raise RuntimeError(f"{url}: {resp['errors']}")
    return resp["data"]


def best_chain(url, max_length):
    q = "{ bestChain(maxLength: %d) { %s } }" % (max_length, BLOCK_FIELDS)
    blocks = [block_from_graphql(b, url) for b in graphql(url, q)["bestChain"]]
    return sorted(blocks, key=lambda b: b["height"])


def block_by_hash(url, state_hash):
    q = '{ block(stateHash: "%s") { %s } }' % (state_hash, BLOCK_FIELDS)
    try:
        b = graphql(url, q)["block"]
    except RuntimeError:
        return None  # not in that node's frontier
    return block_from_graphql(b, url) if b else None


def bucket_url(height, state_hash, network="mainnet"):
    return f"https://storage.googleapis.com/{BUCKET}/{network}-{height}-{state_hash}.json"


def precomputed(height, state_hash, keep=None):
    """OCaml's precomputed block from the public bucket, or None if absent."""
    url = bucket_url(height, state_hash)
    try:
        raw = http(url, timeout=120)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise
    doc = json.loads(raw)
    if keep is not None:
        keep[f"precomputed-{height}-{state_hash}"] = doc
    data = doc.get("data", doc)
    return block_from_protocol_state(data["protocol_state"], state_hash, url)


def bucket_hashes_at(height, network="mainnet"):
    """State hashes of all blocks the bucket has at a height."""
    query = urllib.parse.urlencode(
        {"prefix": f"{network}-{height}-", "fields": "items(name)", "maxResults": "100"}
    )
    page = json.loads(http(f"https://storage.googleapis.com/storage/v1/b/{BUCKET}/o?{query}"))
    names = [item["name"] for item in page.get("items", [])]
    return [n.split("-", 2)[2][: -len(".json")] for n in names if n.endswith(".json")]


def read_protocol_state_zst(path):
    """The `protocol_state` of a zstd precomputed block, decompressing only
    the start of the file."""
    proc = subprocess.Popen(
        ["zstd", "-dc", "--", path], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    )
    buf = b""
    while b'"protocol_state_proof"' not in buf and len(buf) < (8 << 20):
        chunk = proc.stdout.read(65536)
        if not chunk:
            break
        buf += chunk
    proc.kill()
    proc.wait()
    start = buf.index(b'"protocol_state":') + len(b'"protocol_state":')
    end = buf.index(b',"protocol_state_proof"', start)
    return json.loads(buf[start:end])


# --- offline -------------------------------------------------------------------


def offline(args):
    report = Report()
    chain = []
    with open(args.chain) as f:
        for line in f:
            if line.strip():
                height, state_hash = line.split()
                chain.append((int(height), state_hash))
    chain = [
        (h, s)
        for h, s in chain
        if (args.from_height is None or h >= args.from_height)
        and (args.to_height is None or h <= args.to_height)
    ]

    def load(entry):
        height, state_hash = entry
        path = os.path.join(args.blocks, f"{height}-{state_hash}.json.zst")
        if not os.path.exists(path):
            path = os.path.join(args.blocks, f"{height}-{state_hash}.json")
            with open(path) as f:
                doc = json.load(f)
            ps = doc.get("data", doc)["protocol_state"]
        else:
            ps = read_protocol_state_zst(path)
        return block_from_protocol_state(ps, state_hash, path)

    started = time.time()
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        blocks = list(pool.map(load, chain))
    print(f"read {len(blocks)} blocks in {time.time() - started:.0f} s")

    counts = {}
    failures = []
    for i in range(1, len(blocks)):
        p, c = blocks[i - 1], blocks[i]
        kind, fails = check_pair(p, c)
        counts[kind] = counts.get(kind, 0) + 1
        if fails:
            failures.append((p, c, kind, fails))
        if kind == "new epoch":
            emitted = i < 2 or p["snarked_ledger_hash"] != blocks[i - 2]["snarked_ledger_hash"]
            print(f"\nboundary into epoch {c['epoch']} ({utc(slot_time(c['slot_since_hard_fork']))}):")
            print_block("P", p)
            print_block("T", c)
            print(
                f"  slots skipped across the boundary: {c['slot_since_hard_fork'] - p['slot_since_hard_fork'] - 1};"
                f" T.next.ledger == P.snarked: {c['next']['ledger_hash'] == p['snarked_ledger_hash']};"
                f" P emitted a proof: {emitted}"
            )
            report.check(f"epoch {c['epoch']} transition follows the rules", not fails, "; ".join(fails))
    print()
    report.check(
        f"{len(blocks) - 1} consecutive pairs follow the epoch rules {counts}",
        not failures,
        "; ".join(f"{c['height']}: {f}" for p, c, k, f in failures[:5]),
    )
    return report


# --- live ----------------------------------------------------------------------


def live(args):
    report = Report()
    keep = {}
    node = args.node.rstrip("/")
    node_gql = node + "/graphql"
    now = time.time()
    now_slot = int((now - GENESIS_TIMESTAMP) // SLOT_SECONDS)
    print(f"now {utc(now)}, wall-clock slot since fork {now_slot}"
          f" (epoch {now_slot // SLOTS_PER_EPOCH} slot {now_slot % SLOTS_PER_EPOCH})")

    # 1. Status.
    status = json.loads(http(node + "/status"))
    keep["node-status"] = status
    sync = status["transition_frontier"]["sync"]
    report.check("node synced", sync.get("phase") == "Synced", f"phase {sync.get('phase')}")

    # 2. The node's best chain, oldest first.
    chain = best_chain(node_gql, K + 10)
    keep["node-best-chain"] = chain
    root, tip = chain[0], chain[-1]
    print(f"node frontier: root {root['height']} (epoch {root['epoch']} slot {root['slot']}),"
          f" tip {tip['height']} {tip['state_hash']} (epoch {tip['epoch']} slot {tip['slot']})")
    failures, transitions, counts = [], [], {}
    for p, c in zip(chain, chain[1:]):
        kind, fails = check_pair(p, c)
        counts[kind] = counts.get(kind, 0) + 1
        if fails:
            failures.append(f"{c['height']} {short(c['state_hash'])}: " + "; ".join(fails))
        if kind == "new epoch":
            transitions.append((p, c))
    report.check(
        f"node's best chain follows the epoch rules ({len(chain) - 1} pairs, {counts})",
        not failures,
        " | ".join(failures[:3]),
    )

    # 3. The boundary.
    p = t = None
    if transitions:
        p, t = transitions[-1]
        print(f"\nboundary into epoch {t['epoch']} is in the node's frontier:")
        print_block("P (last block of the old epoch)", p)
        print_block("T (first block of the new epoch)", t)
        blocks_to_root = t["height"] + K - tip["height"]
        rate = (tip["height"] - root["height"]) / max(1, tip["slot_since_hard_fork"] - root["slot_since_hard_fork"])
        eta = now + blocks_to_root / max(rate, 1e-6) * SLOT_SECONDS
        fast, slow = (now + blocks_to_root / r * SLOT_SECONDS for r in PAST_EPOCH_RATES)
        report.info(
            "root has not crossed yet",
            f"T becomes the root in about {blocks_to_root} blocks, around {utc(eta)}"
            f" ({rate:.3f} blocks per slot; {utc(fast)} to {utc(slow)} at the rates of past"
            f" epochs). The node builds the new next epoch ledger"
            f" {t['next']['ledger_hash']} (P's snarked ledger) when its root reaches the"
            f" block that emitted it, at or before P.",
        )
    elif root["epoch"] == tip["epoch"] and tip["epoch"] > 0:
        # The boundary is older than the frontier: find P and T in the bucket.
        p_hash = root["next"]["start_checkpoint"]
        p_height = root["height"] - root["next"]["epoch_length"]
        report.info(
            f"boundary into epoch {tip['epoch']} is behind the root",
            f"P should be {p_height} {p_hash} (root.next.start_checkpoint)",
        )
        if not args.no_public:
            try:
                p = precomputed(p_height, p_hash, keep)
            except Exception as e:
                report.warn("bucket fetch for P failed", str(e)[:200])
                p = None
            if p is None:
                report.warn("P not in the bucket", bucket_url(p_height, p_hash))
            else:
                children = []
                try:
                    for h in bucket_hashes_at(p_height + 1):
                        c = precomputed(p_height + 1, h, keep)
                        if c and c["prev_hash"] == p_hash and c["epoch"] > p["epoch"]:
                            children.append(c)
                except Exception as e:
                    report.warn("bucket listing for T failed", str(e)[:200])
                for c in children:
                    kind, fails = check_pair(p, c)
                    report.check(
                        f"bucket block {short(c['state_hash'])} at {c['height']} after P follows the new-epoch rules",
                        not fails,
                        "; ".join(fails),
                    )
                consistent = [c for c in children if c["staking"] == root["staking"]]
                if len(consistent) == 1:
                    t = consistent[0]
                    print_block("P (bucket)", p)
                    print_block("T (bucket)", t)
                report.check(
                    "node root's staking data equal T's (from the bucket)",
                    bool(consistent),
                    f"{len(children)} candidate(s) for T",
                )
    else:
        report.info("no epoch boundary since the node's root")

    # Timing and the prediction for the next boundary.
    next_epoch = tip["epoch"] + 1
    boundary_slot = next_epoch * SLOTS_PER_EPOCH
    seed_final_slot = tip["epoch"] * SLOTS_PER_EPOCH + SLOTS_PER_EPOCH * 2 // 3
    # Blocks per slot over the frontier, and the range of whole epochs so far.
    rate = (tip["height"] - root["height"]) / max(1, tip["slot_since_hard_fork"] - root["slot_since_hard_fork"])
    k_hours = {r: K / r * SLOT_SECONDS / 3600 for r in (rate, *PAST_EPOCH_RATES)}
    report.info(
        f"next boundary: epoch {next_epoch} at slot {boundary_slot}"
        f" (since genesis {GENESIS_SLOT_SINCE_GENESIS + boundary_slot})",
        f"{utc(slot_time(boundary_slot))}; seed and lock checkpoint fixed from slot"
        f" {seed_final_slot}, {utc(slot_time(seed_final_slot))}, and final once the root passes"
        f" that slot, about {k_hours[rate]:.0f} h later at the frontier's {rate:.3f} blocks per slot"
        f" ({k_hours[PAST_EPOCH_RATES[0]]:.0f} to {k_hours[PAST_EPOCH_RATES[1]]:.0f} h at"
        f" {PAST_EPOCH_RATES[0]} to {PAST_EPOCH_RATES[1]}, the range of past epochs). The first"
        f" block of epoch {next_epoch} becomes the root as long after the boundary.",
    )
    seed_fixed = tip["slot"] >= SLOTS_PER_EPOCH * 2 // 3
    prediction = {
        "epoch": next_epoch,
        "staking": {f: tip["next"][f] for f in EPOCH_FIELDS if f != "epoch_length"},
        "min_epoch_length": tip["next"]["epoch_length"],
        "from_block": tip["state_hash"],
        # The seed and lock checkpoint come from the last block of the seed
        # update range on the best chain. Once the tip is past the range they
        # are fixed unless a fork from before that block wins; once the root
        # is past it they are final.
        "seed_fixed": seed_fixed,
        "seed_final": seed_fixed and root["slot_since_hard_fork"] >= seed_final_slot,
        "saved_at": utc(now),
    }
    if seed_fixed:
        report.info(
            f"epoch {next_epoch} staking data will be"
            + ("" if prediction["seed_final"] else " (not final until the root passes the seed update range)"),
            json.dumps(prediction["staking"]) + f", epoch_length >= {prediction['min_epoch_length']}",
        )
    if args.save_prediction:
        with open(args.save_prediction, "w") as f:
            json.dump(prediction, f, indent=1)
        state = "final" if prediction["seed_final"] else "fixed, not final" if seed_fixed else "not fixed yet"
        report.info("prediction saved", f"{args.save_prediction} (seed {state})")
    if args.prediction:
        with open(args.prediction) as f:
            want = json.load(f)
        if not want.get("seed_final"):
            report.warn(
                "the prediction was saved before the seed was final",
                f"saved {want.get('saved_at', '?')} from {want['from_block']}; a mismatch in seed or"
                " lock_checkpoint may be a reorg, not a bug: compare with a later save",
            )
        got = next((b for b in chain if b["epoch"] == want["epoch"]), None)
        if got is None:
            report.info("prediction not checkable yet", f"no block of epoch {want['epoch']} in the frontier")
        else:
            diffs = [
                f"{k}: {got['staking'][k]} != {v}" for k, v in want["staking"].items() if got["staking"][k] != v
            ]
            if got["staking"]["epoch_length"] < want["min_epoch_length"]:
                diffs.append(f"epoch_length {got['staking']['epoch_length']} < {want['min_epoch_length']}")
            report.check(f"epoch {want['epoch']} staking data match the prediction", not diffs, "; ".join(diffs))

    # 4. OCaml's view.
    interesting = [("tip", tip)] + ([("P", p), ("T", t)] if p and t and p["source"] == node_gql else [])
    if not args.no_public:
        for url in args.ocaml_graphql:
            try:
                theirs = best_chain(url, 10)
            except Exception as e:
                report.warn(f"OCaml GraphQL {url} unreachable", str(e)[:200])
                continue
            keep[f"ocaml-best-chain {url}"] = theirs
            their_tip = theirs[-1]
            ours_by_height = {b["height"]: b for b in chain}
            common = [b for b in theirs if b["height"] in ours_by_height]
            same = [b for b in common if ours_by_height[b["height"]]["state_hash"] == b["state_hash"]]
            report.check(
                f"tip agrees with {url}",
                bool(common) and len(same) >= len(common) - 1,
                f"ours {tip['height']} {short(tip['state_hash'])}, theirs {their_tip['height']}"
                f" {short(their_tip['state_hash'])}, {len(same)}/{len(common)} common heights agree",
            )
            for b in same:
                diffs = compare(ours_by_height[b["height"]], b)
                if diffs:
                    report.check(f"block {b['height']} fields equal {url}", False, "; ".join(diffs))
            for label, blk in interesting:
                try:
                    theirs_b = block_by_hash(url, blk["state_hash"])
                except Exception as e:
                    report.warn(f"{label} from {url} failed", str(e)[:200])
                    continue
                if theirs_b is None:
                    report.info(f"{label} not in {url}'s frontier")
                    continue
                keep[f"ocaml-block {url} {blk['state_hash']}"] = theirs_b
                diffs = compare(blk, theirs_b)
                report.check(f"{label} ({blk['height']}) equals {url}'s block", not diffs, "; ".join(diffs))
        for label, blk in interesting:
            try:
                theirs_b = precomputed(blk["height"], blk["state_hash"], keep)
            except Exception as e:
                report.warn(f"bucket fetch for {label} failed", str(e)[:200])
                continue
            if theirs_b is None:
                report.info(f"{label} not (yet) in the precomputed bucket", bucket_url(blk["height"], blk["state_hash"]))
                continue
            diffs = compare(blk, theirs_b)
            report.check(f"{label} ({blk['height']}) equals OCaml's precomputed block", not diffs, "; ".join(diffs))

    # 5. The node holds the staking ledger of its tip.
    if not args.no_ledger_probe:
        probe = t if t and t["source"] == node_gql else tip
        q = '{ account(publicKey: "%s") { delegators { publicKey } } }' % probe["creator"]
        started = time.time()
        try:
            account = graphql(node_gql, q)["account"]
            delegators = [d["publicKey"] for d in (account or {}).get("delegators") or []]
            report.check(
                f"node's staking ledger {short(tip['staking']['ledger_hash'])} has {short(probe['winner'])}"
                f" delegating to {short(probe['creator'])} (winner of block {probe['height']})",
                probe["winner"] in delegators,
                f"{len(delegators)} delegators, {time.time() - started:.1f} s",
            )
        except Exception as e:
            report.check("staking ledger probe", False, str(e)[:200])

    # 6. Bootstraps, ledger syncs and restarts.
    stats = json.loads(http(node + "/stats/sync?limit=256")) or []
    keep["node-sync-stats-kinds"] = [
        (s["kind"], s["best_tip_received"], s["synced"], s["resyncs"]) for s in stats
    ]
    summary = sync_stats_summary(stats)
    keep["node-sync-stats-summary"] = summary
    report.info(
        f"/stats/sync: {summary['snapshots']} snapshots",
        f"oldest {utc_ns(summary['oldest'])} (the node keeps the last 256)",
    )
    if not summary["bootstraps"]:
        report.info("bootstrap", "no Bootstrap snapshot left in /stats/sync")
    for b in summary["bootstraps"]:
        report.info(
            "bootstrap",
            f"target received {utc_ns(b['started'])}, synced {utc_ns(b['synced'])}"
            + (f" ({b['retargets']} earlier target(s) replaced)" if b["retargets"] else ""),
        )
    for f in summary["ledger_fetches"]:
        report.info(
            f"ledger fetched: {f['kind']} {short(f['hash'])}",
            f"from {utc_ns(f['start'])}" + (" (after the last bootstrap synced)" if f["after_bootstrap"] else ""),
        )
    for r in summary["resync_labels"]:
        # `stats_sync.rs` labels a new sync target that arrives before the
        # previous one finished, by comparing the new tip with the ledgers
        # synced at bootstrap. In catch-up that is a label, not a refetch:
        # the ledger fetches above show what was actually fetched.
        report.info(f"resync label {r['kind']}", f"at {utc_ns(r['time'])}")
    if t is not None and stats:
        # A restart or a new bootstrap after the boundary shows as a Bootstrap
        # snapshot after it, or, once that snapshot has been evicted, as
        # ledger fetches after it (every snapshot carries the latest fetch of
        # each ledger kind).
        boundary_time = slot_time(t["epoch"] * SLOTS_PER_EPOCH)
        late_bootstraps = [b for b in summary["bootstraps"] if b["started"] / 1e9 >= boundary_time]
        late_fetches = [f for f in summary["ledger_fetches"] if f["start"] / 1e9 >= boundary_time]
        name = f"node stayed synced across the boundary into epoch {t['epoch']} ({utc(boundary_time)})"
        detail = (
            f"bootstraps since: {[utc_ns(b['started']) for b in late_bootstraps] or 'none'};"
            f" ledger fetches since: {[(f['kind'], short(f['hash'])) for f in late_fetches] or 'none'}"
        )
        ok = not late_bootstraps and not late_fetches
        # Once the boundary is behind the root, a node started later never
        # crossed it; that is only a failure for the boundary being watched.
        if t["source"] == node_gql or args.watch_epoch == t["epoch"]:
            report.check(name, ok, detail)
        else:
            report.info(name + ("" if ok else ": no, it started or resynced after it"), detail)

    peers = status.get("peers") or []
    connected = [x for x in peers if x.get("connection_status") == "Connected"]
    agree = [x for x in connected if x.get("best_tip") == tip["state_hash"]]
    report.info("peers", f"{len(connected)} connected, {len(agree)} report our tip as their best tip")

    if args.out:
        os.makedirs(args.out, exist_ok=True)
        path = os.path.join(args.out, f"capture-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}.json")
        with open(path, "w") as f:
            json.dump(keep, f, indent=1, default=str)
        report.info("captured", path)
    return report


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", default="http://localhost:3200", help="mina-rust node HTTP base URL")
    ap.add_argument("--ocaml-graphql", action="append", help="OCaml GraphQL URL (repeatable)")
    ap.add_argument("--no-public", action="store_true", help="skip OCaml GraphQL and the bucket")
    ap.add_argument("--no-ledger-probe", action="store_true", help="skip the delegators query")
    ap.add_argument("--out", help="directory for a JSON capture of what was compared")
    ap.add_argument("--save-prediction", help="write the next epoch's expected staking data")
    ap.add_argument("--prediction", help="check a saved prediction")
    ap.add_argument(
        "--watch-epoch",
        type=int,
        help="the boundary being watched (e.g. 7): once it is behind the root, still fail"
        " if the node bootstrapped or fetched a ledger after it",
    )
    ap.add_argument("--offline", action="store_true", help="check a local chain of precomputed blocks")
    ap.add_argument("--blocks", help="offline: directory of <height>-<hash>.json[.zst]")
    ap.add_argument("--chain", help="offline: chain.txt (<height> <hash> per line)")
    ap.add_argument("--from-height", type=int)
    ap.add_argument("--to-height", type=int)
    ap.add_argument("--jobs", type=int, default=8)
    args = ap.parse_args()
    if not args.ocaml_graphql:
        args.ocaml_graphql = DEFAULT_OCAML_GRAPHQL
    try:
        if args.offline:
            if not (args.blocks and args.chain):
                ap.error("--offline needs --blocks and --chain")
            report = offline(args)
        else:
            report = live(args)
    except Exception as e:
        print(f"error: {e!r}", file=sys.stderr)
        return 2
    print(f"\n{report.passed} passed, {report.failed} failed")
    return 1 if report.failed else 0


if __name__ == "__main__":
    sys.exit(main())
