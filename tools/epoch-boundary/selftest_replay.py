#!/usr/bin/env python3
"""Rehearse check_epoch_boundary.py's live mode on a past boundary.

The node's best chain is replaced with blocks from the replay data, cut so
that a boundary sits inside a 290-block frontier, as it will be for about 20
hours after a live boundary. The OCaml side is the real public bucket: the
precomputed blocks of P, T and the tip are fetched and compared.

What is not real:

- the node's `/status` (always Synced, no peers);
- the node's `/stats/sync`: synthetic snapshots (a bootstrap a day before the
  boundary, then catch-ups), or, in the "restarted" run, a bootstrap after it;
- the staking ledger probe: the delegators list contains the winner by
  construction, so it only exercises the code path;
- the OCaml GraphQL nodes are not queried, as the replayed blocks are older
  than their frontier.

Runs, in order:

1. "before": the frontier ends at P, the last block of the old epoch; the
   prediction of the new epoch's staking data is saved.
2. "clean": the boundary is in the frontier; the prediction is checked.
3. "tampered T": T's staking seed is wrong; must fail the rules, the
   comparison with the bucket and the prediction.
4. "restarted": the clean chain, but the node bootstrapped after the
   boundary; must fail only the stayed-synced check.

    selftest_replay.py --blocks DIR --chain DIR/chain.txt --boundary-height 563796

`--boundary-height` is the height of T, the first block of the new epoch
(563796 is the first block of epoch 6). Exit status 0 if every run did what
it should.
"""

import argparse
import copy
import importlib.util
import json
import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("ceb", os.path.join(HERE, "check_epoch_boundary.py"))
ceb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ceb)

NODE = "http://replay.invalid"


def sync_stats(bootstrap_time, catchup_times, ledgers):
    """`/stats/sync` as the node serves it: newest first, nanoseconds, each
    snapshot repeating the ledgers and resyncs of the one before."""
    ns = lambda t: int(t * 1e9)  # noqa: E731
    snapshots = [
        {
            "kind": "Bootstrap",
            "best_tip_received": ns(bootstrap_time),
            "synced": ns(bootstrap_time + 480),
            "ledgers": ledgers,
            "blocks": [],
            "resyncs": [],
        }
    ]
    resyncs = []
    for i, t in enumerate(catchup_times):
        if i == 1:  # a label, repeated by every later snapshot
            resyncs = [{"kind": "RootLedgerChange", "time": ns(t)}]
        snapshots.append(
            {
                "kind": "Catchup",
                "best_tip_received": ns(t),
                "synced": ns(t + 5),
                "ledgers": ledgers,
                "blocks": [],
                "resyncs": list(resyncs),
            }
        )
    return list(reversed(snapshots))


def ledger_entry(hash_, start):
    ns = lambda t: int(t * 1e9)  # noqa: E731
    return {
        "snarked": {
            "hash": hash_,
            "fetch_hashes_start": ns(start),
            "fetch_hashes_end": ns(start + 100),
            "fetch_accounts_start": ns(start),
            "fetch_accounts_end": ns(start + 110),
        },
        "staged": {
            "hash": None,
            "fetch_parts_start": None,
            "fetch_parts_end": None,
            "reconstruct_start": None,
            "reconstruct_end": None,
        },
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--blocks", required=True)
    ap.add_argument("--chain", required=True)
    ap.add_argument("--boundary-height", type=int, default=563796)
    ap.add_argument("--after", type=int, default=90, help="blocks after T in the frontier")
    args = ap.parse_args()

    entries = {}
    with open(args.chain) as f:
        for line in f:
            if line.strip():
                h, s = line.split()
                entries[int(h)] = s

    def window(tip_height):
        out = []
        for h in range(tip_height - ceb.K, tip_height + 1):
            path = os.path.join(args.blocks, f"{h}-{entries[h]}.json.zst")
            ps = ceb.read_protocol_state_zst(path)
            out.append(ceb.block_from_protocol_state(ps, entries[h], NODE + "/graphql"))
        return out

    before = window(args.boundary_height - 1)
    after = window(args.boundary_height + args.after)
    t = next(b for b in after if b["height"] == args.boundary_height)
    boundary_time = ceb.slot_time(t["epoch"] * ceb.SLOTS_PER_EPOCH)
    root_ledger = after[0]["snarked_ledger_hash"]
    ledgers = {
        "staking_epoch": ledger_entry(before[-1]["staking"]["ledger_hash"], boundary_time - 86400),
        "next_epoch": None,
        "root": ledger_entry(root_ledger, boundary_time - 86400 + 120),
    }
    catchups = [boundary_time - 86000 + 300 * i for i in range(0, 300)]
    stats_synced = sync_stats(boundary_time - 86400, catchups, ledgers)
    restarted_ledgers = {
        "staking_epoch": ledger_entry(t["staking"]["ledger_hash"], boundary_time + 3600),
        "next_epoch": None,
        "root": ledger_entry(root_ledger, boundary_time + 3720),
    }
    stats_restarted = sync_stats(
        boundary_time + 3600, [boundary_time + 4200 + 300 * i for i in range(20)], restarted_ledgers
    )

    real_best_chain, real_http, real_graphql = ceb.best_chain, ceb.http, ceb.graphql
    prediction_file = os.path.join(tempfile.mkdtemp(prefix="r10-selftest-"), "prediction.json")

    def run(label, chain, stats, **opts):
        def best_chain(url, n):
            return copy.deepcopy(chain[-n:]) if url.startswith(NODE) else real_best_chain(url, n)

        def http(url, *a, **kw):
            if url == NODE + "/status":
                status = {"transition_frontier": {"sync": {"phase": "Synced"}}, "peers": []}
                return json.dumps(status).encode()
            if url.startswith(NODE + "/stats/sync"):
                return json.dumps(stats).encode()
            return real_http(url, *a, **kw)

        def graphql(url, q):
            if url.startswith(NODE) and "delegators" in q:
                probe = next((b for b in chain if b["height"] == args.boundary_height), chain[-1])
                return {"account": {"delegators": [{"publicKey": probe["winner"]}]}}
            return real_graphql(url, q)

        ceb.best_chain, ceb.http, ceb.graphql = best_chain, http, graphql
        print(f"\n=== {label} ===")
        ns = argparse.Namespace(
            node=NODE,
            ocaml_graphql=[],
            no_public=False,
            no_ledger_probe=False,
            out=None,
            save_prediction=None,
            prediction=None,
            watch_epoch=None,
        )
        for k, v in opts.items():
            setattr(ns, k, v)
        report = ceb.live(ns)
        print(f"{label}: {report.passed} passed, {report.failed} failed")
        return report

    results = []
    pre = run("before: frontier ends at P", before, stats_synced, save_prediction=prediction_file)
    with open(prediction_file) as f:
        saved = json.load(f)
    results.append(("before passes", pre.failed == 0))
    results.append(("prediction saved with a final seed", saved["seed_final"] and saved["epoch"] == t["epoch"]))

    clean = run("clean: boundary in the frontier", after, stats_synced, prediction=prediction_file)
    results.append(("clean passes", clean.failed == 0))

    tampered = copy.deepcopy(after)
    bad = next(b for b in tampered if b["height"] == args.boundary_height)
    bad["staking"]["seed"] = bad["next"]["seed"]  # a wrong staking seed in T
    dirty = run("tampered T", tampered, stats_synced, prediction=prediction_file)
    results.append(("tampered T fails the rules, the bucket and the prediction", dirty.failed >= 3))

    restarted = run("restarted after the boundary", after, stats_restarted, prediction=prediction_file)
    results.append(("restarted fails only the stayed-synced check", restarted.failed == 1))

    print()
    for name, ok in results:
        print(f"[{'ok' if ok else 'NOT OK'}] {name}")
    ok = all(ok for _, ok in results)
    print(f"\nself-test {'passed' if ok else 'FAILED'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
