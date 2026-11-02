"""Multi-step producer traces for the compiled-OCaml differential runner.

Each trace drives the production block producer through steps that move the
clock and replace the best tip: cold starts after an epoch boundary, restarts
with persisted VRF results, reorgs across the seed lock point and the epoch
boundary, a skipped epoch, a delayed next-epoch ledger and a completed epoch
whose inputs change. Compiled OCaml computes every expected payload: the
epoch data a tip selects for the clock's epoch (select_epoch_data), epoch
data across a boundary (Epoch_data.update_pair) and every VRF output and
eligibility. The runner transcribes no consensus function.

Two expectations are producer policy rather than OCaml output and are stated
per step: a missing staking ledger is waited for (no bound data, no wins),
and the next epoch is evaluated ahead only where a step says so
(`expect_next`). A step may also require that a restart reuses, or does not
reuse, persisted VRF results (`expect_cached`).
"""
import base64
import copy
import hashlib

SLOTS = 7140
# One delegator with two thirds of a fork's total: some slots won, some lost.
STAKE = "2000000000000000000"
TOTAL = 3_000_000_000_000_000_000
ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58check(payload):
    raw = payload + hashlib.sha256(hashlib.sha256(payload).digest()).digest()[:4]
    n = int.from_bytes(raw, "big")
    out = ""
    while n:
        n, digit = divmod(n, 58)
        out = ALPHABET[digit] + out
    return "1" * (len(raw) - len(raw.lstrip(b"\0"))) + out


def b58decode(text):
    n = 0
    for char in text:
        n = n * 58 + ALPHABET.index(char)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big")
    raw = b"\0" * (len(text) - len(text.lstrip("1"))) + raw
    payload, check = raw[:-4], raw[-4:]
    if hashlib.sha256(hashlib.sha256(payload).digest()).digest()[:4] != check:
        raise ValueError("bad base58 checksum")
    return payload


def seed(n):
    """A public synthetic epoch seed (version 0x0d)."""
    return b58check(bytes([0x0D, 0x01]) + n.to_bytes(32, "little"))


def ledger(n):
    """A public synthetic ledger hash (version 0x05)."""
    return b58check(bytes([0x05, 0x01]) + n.to_bytes(32, "little"))


def seed_field(text):
    """The field element of a base58 epoch seed, as the VRF hook takes it."""
    payload = b58decode(text)
    if payload[:2] != bytes([0x0D, 0x01]):
        raise ValueError("not an epoch seed")
    return str(int.from_bytes(payload[2:], "little"))


def project(value):
    """The payload a producer binds: seed, ledger hash and total currency."""
    if value is None:
        return None
    return dict(seed=value["seed"], ledger=value["ledger"]["hash"],
                total_currency=value["ledger"]["total_currency"])


def data(template, n, seed_n=None, ledger_n=None, total=None):
    d = copy.deepcopy(template)
    d["seed"] = seed(100 + n if seed_n is None else seed_n)
    d["ledger"]["hash"] = ledger(200 + n if ledger_n is None else ledger_n)
    d["ledger"]["total_currency"] = str(TOTAL + n if total is None else total)
    return d


class Builder:
    def __init__(self, template, parent_hash, at_slot, ocaml):
        self.template, self.parent_hash = template, parent_hash
        self.at_slot, self.ocaml = at_slot, ocaml

    def tip(self, slot, staking=0, nxt=1, **changed):
        """A tip at `slot` with numbered staking and next epoch data; keyword
        `seed`, `ledger` or `total` give its next epoch data another value,
        as a fork's would."""
        s = self.at_slot(self.template, slot)
        s["staking_epoch_data"] = data(self.template["staking_epoch_data"], staking)
        s["next_epoch_data"] = data(self.template["next_epoch_data"], nxt,
                                    changed.get("seed"), changed.get("ledger"),
                                    changed.get("total"))
        return s

    def winning_key(self, slot, epoch_data):
        """A public synthetic key id (0 to 6) that compiled OCaml says wins
        `slot` with this epoch data, with its truncated VRF output."""
        for key_id in range(7):
            vrf = self.ocaml.query(dict(
                op="vrf", slot=slot, seed=seed_field(epoch_data["seed"]), key_id=key_id,
                index=0, stake=STAKE, total=epoch_data["ledger"]["total_currency"]))
            if vrf["won"]:
                return key_id, bytes.fromhex(vrf["truncated"])
        raise RuntimeError(f"no synthetic key wins slot {slot}")

    def child(self, parent, slot):
        """The child of `parent` at `slot`, its epoch data from compiled
        OCaml's Epoch_data.update_pair."""
        update = self.ocaml.query(dict(
            op="epoch_update", state=parent, next=slot, parent_hash=self.parent_hash,
            vrf_hash="1", snarked=ledger(777), genesis=ledger(1),
            total=str(TOTAL + 7)))
        s = self.at_slot(self.template, slot)
        s["staking_epoch_data"] = update["staking"]
        s["next_epoch_data"] = update["next"]
        s["epoch_count"] = str(update["count"])
        return s


def blake2b(data):
    return hashlib.blake2b(data, digest_size=32).digest()


def low_vrf_output():
    """A truncated VRF output whose Blake2b digest, which fork choice
    compares for blocks of one slot, is the lowest of 4,096 candidates: a
    same-slot sibling carrying it does not outrank the producer's win."""
    candidates = (bytes([i % 256, i // 256]) + bytes(30) for i in range(4096))
    return min(candidates, key=blake2b)


def trace(name, *steps, key_id=0, index=0):
    return dict(name=name, case=dict(op="trace", key_id=key_id,
                delegators=[dict(index=index, stake=STAKE)], steps=list(steps)))


def traces(builder):
    b = builder
    # Cold starts: the clock in the epoch after the tip's, before any block
    # of it, across the seed lock point and both ends of the epoch.
    for tip_slot in [1, 4759, 4760, 7139, SLOTS + 1]:
        for offset in [0, 1, 77]:
            epoch = tip_slot // SLOTS
            clock = (epoch + 1) * SLOTS + offset
            yield trace(f"cold-start-{tip_slot}-{offset}",
                        dict(clock=clock, tip=b.tip(tip_slot, epoch, epoch + 1)),
                        key_id=offset % 7, index=tip_slot % 5)
    for key_id in [0, 1]:
        a = b.tip(4800, 0, 1)
        # Restart with persisted results: the same wins, from the cache.
        yield trace(f"restart-persisted-{key_id}",
                    dict(clock=SLOTS + 2, tip=a),
                    dict(clock=SLOTS + 2, offset_s=30, restart=True, expect_cached=True),
                    key_id=key_id)
        # A missing ledger persists nothing; a restart with it evaluates.
        yield trace(f"unavailable-then-restart-{key_id}",
                    dict(clock=SLOTS + 2, tip=a, ledger=False),
                    dict(clock=SLOTS + 2, offset_s=30, restart=True, expect_cached=False),
                    key_id=key_id)
        # The next-epoch ledger arrives late: asked again after the retry
        # interval without a new tip, then reused after a restart.
        yield trace(f"delayed-ledger-{key_id}",
                    dict(clock=SLOTS + 2, tip=a, ledger=False),
                    dict(clock=SLOTS + 2, offset_s=20),
                    dict(clock=SLOTS + 3, restart=True, expect_cached=True),
                    key_id=key_id)
    # Reorgs across the seed lock point, early evaluation: a tip before the
    # lock, then another fork's tip after it with another next seed, ledger
    # or total; the old evaluation's response arrives after the switch.
    for changed in ["seed", "ledger", "total"]:
        before = b.tip(4759, 0, 1)
        after = b.tip(4800, 0, 1, **{changed: 900})
        yield trace(f"seed-lock-reorg-{changed}",
                    dict(clock=SLOTS + 4, tip=before),
                    dict(clock=SLOTS + 5, tip=after),
                    dict(clock=SLOTS + 6, tip=before))
    # The same with precomputation in the tip's epoch: the root reached the
    # tip's epoch and the seed is final; the reorg then crosses into the
    # precomputed epoch.
    root = b.tip(200, 0, 1)
    for changed in ["seed", "ledger"]:
        a = b.tip(7000, 0, 1)
        fork = b.tip(7001, 0, 1, **{changed: 901})
        yield trace(f"seed-lock-reorg-precomputed-{changed}",
                    dict(clock=SLOTS - 4, root=root, tip=a),
                    dict(clock=SLOTS - 1, tip=a, expect_next=True),
                    dict(clock=SLOTS - 1, offset_s=30, tip=fork, expect_next=True),
                    dict(clock=SLOTS + 1, tip=fork))
    # A reorg across the epoch boundary: the first block of the epoch, then
    # a fork of the previous epoch with other next data, then back.
    parent = b.tip(7000, 0, 1)
    first = b.child(parent, SLOTS + 1)
    for changed in ["seed", "total"]:
        fork = b.tip(7100, 0, 1, **{changed: 902})
        yield trace(f"boundary-reorg-{changed}",
                    dict(clock=SLOTS + 3, tip=first),
                    dict(clock=SLOTS + 4, tip=fork),
                    dict(clock=SLOTS + 5, tip=first))
    # A skipped epoch: no data two epochs past the tip, until a tip of the
    # clock's epoch arrives.
    one = b.tip(SLOTS + 860, 1, 2)
    yield trace("skipped-epoch",
                dict(clock=3 * SLOTS + 5, tip=one),
                dict(clock=3 * SLOTS + 6, tip=b.child(one, 3 * SLOTS + 2)))
    # A win of epoch 2 evaluated on a fork with a block in epoch 1, then a
    # best tip of the win's own slot on a branch with no block in epoch 1.
    # The block is made on that tip's parent, two epochs back: OCaml's
    # update_pair gives the child its next epoch data, which the win used,
    # though select_epoch_data gives the parent none for epoch 2.
    win = 2 * SLOTS + 5
    parent = b.tip(7130, 0, 1)
    fork = b.tip(SLOTS + 5000, 1, 1)
    assert fork["next_epoch_data"]["seed"] == parent["next_epoch_data"]["seed"]
    key_id, truncated = b.winning_key(win, parent["next_epoch_data"])
    sibling = b.child(parent, win)
    low = low_vrf_output()
    if blake2b(truncated) <= blake2b(low):
        raise RuntimeError("sibling VRF output would outrank the win")
    sibling["last_vrf_output"] = base64.urlsafe_b64encode(low).decode()
    yield trace("same-slot-after-empty-epoch",
                dict(clock=win - 1, tip=fork),
                dict(clock=win, root=parent, tip=sibling),
                key_id=key_id)
    # A completed epoch whose inputs change: evaluated to its last slot with
    # one tip, then evaluated again for another tip's data.
    for changed in ["seed", "ledger", "total"]:
        yield trace(f"completed-changed-{changed}",
                    dict(clock=2 * SLOTS - 4, tip=b.tip(6000, 0, 1)),
                    dict(clock=2 * SLOTS - 4, offset_s=40,
                         tip=b.tip(6001, 0, 1, **{changed: 903})))


def expected(ocaml, case):
    """Each step's expectation, from compiled OCaml."""
    out, tip = [], None
    key_id = case["key_id"]
    (delegator,) = case["delegators"]
    for step in case["steps"]:
        tip = step.get("tip") or tip
        clock = step["clock"]
        epoch = clock // SLOTS
        # get_epoch_data_for_vrf evaluates the clock's epoch, from the next
        # slot when the clock is in the tip's own slot; steps keep that slot
        # within the epoch.
        if (int(tip["curr_global_slot_since_hard_fork"]["slot_number"]) == clock
                and (clock + 1) // SLOTS != epoch):
            raise RuntimeError("trace step in the tip's own last slot of an epoch")
        ledger_available = step.get("ledger", True)
        window_end = min(clock + step.get("window", 6), (epoch + 1) * SLOTS)
        selected = project(ocaml.query(dict(op="epoch_select", state=tip, epoch=epoch)))
        bound = selected if ledger_available else None
        nxt = None
        if step.get("expect_next") and ledger_available:
            nxt = project(ocaml.query(dict(op="epoch_select", state=tip, epoch=epoch + 1)))
        wins = []
        if bound is not None:
            for slot in range(clock, window_end):
                vrf = ocaml.query(dict(op="vrf", slot=slot, seed=seed_field(bound["seed"]),
                                       key_id=key_id, index=delegator["index"],
                                       stake=delegator["stake"], total=bound["total_currency"]))
                if vrf["won"]:
                    wins.append(dict(slot=slot, hash=vrf["hash"]))
        expectation = dict(bound=bound, next_bound=nxt, wins=wins,
                           produce=clock if any(w["slot"] == clock for w in wins) else None,
                           violations=[])
        if "expect_cached" in step:
            expectation["cached"] = step["expect_cached"]
        out.append(expectation)
    return out


def rust_view(observed, expectation):
    """The Rust step observation in the expectation's terms."""
    observed = dict(observed)
    hits = observed.pop("cache_hits")
    if "cached" in expectation:
        observed["cached"] = hits > 0
    return observed
