#!/usr/bin/env python3
"""Compare compiled OCaml and production Rust, retaining every public input."""
import argparse
import base64
import collections
import copy
import hashlib
import json
import os
from pathlib import Path
import random
import selectors
import subprocess
import time

import traces

ROOT = Path(__file__).resolve().parents[2]
SEED = 0xB003403


def canonical(value):
    # Preserve types, leading zeros and every byte of encoded hashes/VRFs.
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def oracle_view(case, value):
    # Compare exactly the payload actually carried by Rust EpochContext.
    # Checkpoints/epoch length are not part of that context.
    if case["op"] == "epoch_select" and value is not None:
        return dict(seed=value["seed"], ledger=value["ledger"]["hash"],
                    total_currency=value["ledger"]["total_currency"])
    return value


class Worker:
    def __init__(self, command, output, network, name):
        self.log = (output / f"{network}-{name}.stderr").open("w")
        env = dict(os.environ, MINA_PROFILE=network, TMPDIR=str(output / "tmp"))
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=self.log, text=True, env=env, bufsize=1)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)

    def query(self, case):
        self.process.stdin.write(json.dumps(case, separators=(",", ":")) + "\n")
        self.process.stdin.flush()
        if not self.selector.select(60):
            raise RuntimeError("adapter response timed out")
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError(f"adapter exited: {self.process.poll()}")
        value = json.loads(line)
        if isinstance(value, dict) and "error" in value:
            raise RuntimeError(value["error"])
        return value

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.selector.close()
        self.log.close()
        return self.process.returncode


def state_hash(n):
    """Public synthetic state-hash fields, encoded with the protocol prefix."""
    raw = bytes.fromhex("1001") + n.to_bytes(32, "little")
    raw += hashlib.sha256(hashlib.sha256(raw).digest()).digest()[:4]
    n = int.from_bytes(raw, "big")
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    out = ""
    while n:
        n, digit = divmod(n, 58)
        out = alphabet[digit] + out
    return out


def base_state():
    path = sorted((ROOT / "tests/files/forks").glob("long-keep-density-161-166-*-tip.json"))[0]
    data = json.loads(path.read_text())
    state = data["body"]["consensus_state"]
    state["curr_global_slot_since_hard_fork"]["slots_per_epoch"] = "7140"
    state["next_epoch_data"]["ledger"]["hash"] = "jxTAZfKKDxoX4vtt68pQCWooXoVLjnfBpusaMwewrcZxsL3uWp6"
    return state, data["previous_state_hash"], data["body"]["genesis_state_hash"]


def at_slot(template, slot):
    s = copy.deepcopy(template)
    s["curr_global_slot_since_hard_fork"]["slot_number"] = str(slot)
    s["epoch_count"] = str(slot // 7140)
    s["global_slot_since_genesis"] = str(slot)
    return s


def cases(scale):
    rng = random.Random(SEED)
    template, h1, h2 = base_state()
    edges = [0, 1, 6, 7, 8, 76, 77, 78, 2159, 2160, 2236, 2237, 2238,
             4758, 4759, 4760, 4761, 7138, 7139, 7140, 7141, 14279, 14280,
             21420, 28560, 2**32 - 1]
    for slot in edges + [rng.randrange(2**32) for _ in range(1000 * scale)]:
        yield "arithmetic", dict(op="arithmetic", slot=slot)
        yield "epoch_bounds", dict(op="epoch_bounds", slot=slot)
    for i in range(2000 * scale):
        prev = rng.choice(edges[:-1]) if i < 400 else rng.randrange(40000)
        nxt = prev + rng.choice([0, 1, 6, 7, 8, 70, 76, 77, 78, 7140, 14280])
        yield "density", dict(op="density", prev=prev, next=nxt,
            buckets=[rng.randrange(8) for _ in range(11)], minimum=rng.randrange(78), incr=bool(i % 2))
    for i in range(6000 * scale):
        p = rng.choice(edges[:-1]) if i < 1500 else rng.randrange(60000)
        q = p + rng.choice([0, 1, 6, 7, 76, 77, 78, 4759, 7140, 14280])
        a, b = at_slot(template, p), at_slot(template, q)
        for s in [a, b]:
            s["epoch_count"] = str(rng.randrange(9))  # differs after empty epochs
            s["blockchain_length"] = str(rng.randrange(1, 8))
            s["min_window_density"] = str(rng.randrange(78))
            s["sub_window_densities"] = [str(rng.randrange(8)) for _ in range(11)]
            s["staking_epoch_data"]["lock_checkpoint"] = rng.choice([h1, h2])
            s["next_epoch_data"]["lock_checkpoint"] = rng.choice([h1, h2])
            s["last_vrf_output"] = base64.urlsafe_b64encode(bytes([rng.randrange(3)] * 32)).decode()
        yield "selection_generated", dict(op="select", a=a, b=b, a_hash=state_hash(rng.randrange(2**254)), b_hash=state_hash(rng.randrange(2**254)))
    tie_state = at_slot(template, 7140)
    for a in [0,1,255,256,2**32,2**64,2**128,2**254-1]:
        for b in [0,1,255,256,2**32,2**64,2**128,2**254-1]:
            yield "selection_hash_ties", dict(op="select",a=tie_state,b=tie_state,
                a_hash=state_hash(a),b_hash=state_hash(b))
    for prev in edges[:-1]:
        s = at_slot(template, prev)
        s["epoch_count"] = str(max(0,prev//7140-2))
        for epoch in [prev // 7140, prev // 7140 + 1, prev // 7140 + 2]:
            area = "epoch_start_deferred" if epoch == prev // 7140 + 1 else "epoch_selection"
            yield area, dict(op="epoch_select", state=s, epoch=epoch)
        yield "epoch_precompute", dict(op="epoch_select", state=s, epoch=prev // 7140 + 1, precompute=True)
        for gap in [1, 7, 77, 7140, 14280]:
            for genesis in [True, False]:
                yield "epoch_update", dict(op="epoch_update", state=s, next=prev+gap,
                    parent_hash=h1, vrf_hash=str(rng.randrange(2**32)),
                    snarked=s["next_epoch_data"]["ledger"]["hash"],
                    genesis=s["next_epoch_data" if genesis else "staking_epoch_data"]["ledger"]["hash"],
                    total="1000000000000000000")
    for i in range(512 * scale):
        total = rng.choice([1, 10**9, 10**18, 2**64 - 1])
        stake = rng.choice([0, 1, total // 2, total - 1, total])
        yield "vrf", dict(op="vrf", slot=rng.choice(edges), seed=str(rng.choice([0,1,2**254-1,rng.randrange(2**254)])),
            key_id=i % 7, index=rng.choice([0, 1, 2, 31, 255, 2**35-1]), stake=str(stake), total=str(total))
    for i in range(32 * scale):
        accounts = []
        for owner in range(i % 17):
            accounts.append(dict(owner=owner, token=rng.choice([1,1,1,2,7]),
                delegate=rng.choice([None, 0, 0, 1, owner]), balance=str(rng.choice([0,1,10**9,2**64-1]))))
        yield "delegation", dict(op="delegation", producer=i % 3, accounts=accounts)


def recorded(corpus, network):
    """Load saved protocol states only, leaving unrelated transaction fields alone."""
    chain = corpus / ("replay" if network == "mainnet" else "replay-devnet") / "chain.txt"
    blocks = chain.parent / "blocks" if network == "mainnet" else chain.parent
    names = chain.read_text().splitlines()
    if len(names) < 2:
        raise RuntimeError("recorded chain contains fewer than two blocks")
    selected = sorted(set(range(min(32,len(names)-1))) | set(range(max(0,len(names)-33),len(names)-1)))
    cache = {}
    def load(i):
        if i in cache:
            return cache[i]
        name = names[i].replace(" ", "-")
        path = blocks / (name + ".json")
        if path.exists(): raw = path.read_bytes()
        else: raw = subprocess.check_output(["zstd", "-dc", str(path)+".zst"])
        # Some saved transaction memos contain non-UTF-8 bytes. Only the
        # protocol state enters this gate, and it must decode without loss.
        raw = raw.decode("utf-8", errors="replace")
        marker = raw.index('"protocol_state"')
        start = raw.index("{",marker)
        data, _ = json.JSONDecoder().raw_decode(raw[start:])
        if "\ufffd" in json.dumps(data, ensure_ascii=False):
            raise RuntimeError("invalid UTF-8 in recorded protocol state")
        cache[i] = (data, name.split("-")[-1])
        return cache[i]
    for i in selected:
        (a, ah), (b, bh) = load(i), load(i+1)
        if b["previous_state_hash"] != ah:
            raise RuntimeError(f"recorded parent linkage failed at {i}")
        a, b = a["body"]["consensus_state"], b["body"]["consensus_state"]
        for x,y,xh,yh in [(a,b,ah,bh),(b,a,bh,ah)]:
            yield "selection_recorded", dict(op="select",a=x,b=y,a_hash=xh,b_hash=yh)


def recorded_forks(directory, network):
    groups = collections.defaultdict(list)
    for path in sorted(directory.glob(network+"-*.json")):
        raw = path.read_text()
        full = json.loads(raw)
        if full["data"]["protocol_version"]["transaction"] != 4:
            continue
        start = raw.index("{",raw.index('"protocol_state"'))
        protocol, _ = json.JSONDecoder().raw_decode(raw[start:])
        groups[protocol["previous_state_hash"]].append((protocol["body"]["consensus_state"],path.stem.split("-")[-1]))
    count = 0
    for siblings in groups.values():
        for i,(a,ah) in enumerate(siblings):
            for b,bh in siblings[i+1:]:
                for x,y,xh,yh in [(a,b,ah,bh),(b,a,bh,ah)]:
                    count += 1
                    yield "selection_recorded_forks", dict(op="select",a=x,b=y,a_hash=xh,b_hash=yh)
    if not count:
        raise RuntimeError("no recorded Mesa sibling pairs for "+network)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust", type=Path, required=True)
    parser.add_argument("--rust-trace", type=Path,
                        help="producer_trace adapter, for the epoch_traces area")
    parser.add_argument("--ocaml", type=Path, required=True)
    parser.add_argument("--ocaml-executable", type=Path, help="underlying executable when --ocaml is a wrapper")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--corpus", type=Path)
    parser.add_argument("--forks", type=Path)
    parser.add_argument("--scale", type=int, default=1)
    parser.add_argument("--network", choices=["mainnet","devnet","both"],default="both")
    parser.add_argument("--only", help="comma-separated coverage areas")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "tmp").mkdir()
    report = dict(seed=SEED, binaries={n:hashlib.sha256(p.read_bytes()).hexdigest() for n,p in [("rust",args.rust),("ocaml",args.ocaml)]}, networks={})
    if args.rust_trace:
        report["binaries"]["rust_trace"] = hashlib.sha256(args.rust_trace.read_bytes()).hexdigest()
    if args.ocaml_executable:
        report["binaries"]["ocaml_executable"] = hashlib.sha256(args.ocaml_executable.read_bytes()).hexdigest()
    failures = []
    started = time.monotonic()
    networks = ["mainnet","devnet"] if args.network == "both" else [args.network]
    try:
        for network in networks:
            workers = []
            try:
                rust = Worker([str(args.rust.resolve()),network],output,network,"rust"); workers.append(rust)
                ocaml = Worker([str(args.ocaml.resolve())],output,network,"ocaml"); workers.append(ocaml)
                counts = collections.Counter()
                report["networks"][network] = dict(counts=counts, rust_build=rust.query({"op":"build_info"}), constants={"rust":rust.query({"op":"constants"}),"ocaml":ocaml.query({"op":"constants"})})
                with (output / f"{network}.jsonl").open("w") as log:
                    def compare(area, case, expected=None):
                        try:
                            observed = rust.query(case)
                            expected = ocaml.query(case) if expected is None else expected
                        except Exception as error:
                            log.write(json.dumps(dict(area=area,case=case,adapter_error=str(error)))+"\n")
                            log.flush()
                            raise
                        expected = oracle_view(case, expected)
                        equal = canonical(observed) == canonical(expected)
                        counts[area] += 1
                        record = dict(area=area,case=case,rust=observed,ocaml=expected,equal=equal)
                        log.write(json.dumps(record,separators=(",",":"))+"\n")
                        if not equal:
                            failures.append(dict(network=network, **record))
                    compare("constants", {"op":"constants"})
                    selected = set(args.only.split(",")) if args.only else None
                    for area,case in cases(args.scale):
                        if selected is None or area in selected: compare(area,case)
                    if selected is None or selected & {"regressions", "epoch_start_deferred"}:
                        fixtures = json.loads((Path(__file__).parent / "regressions.json").read_text())
                        for fixture in fixtures["cases"]:
                            area = fixture.get("area", "regressions")
                            if selected is not None and area not in selected:
                                continue
                            live = ocaml.query(fixture["case"])
                            if canonical(live) != canonical(fixture["ocaml"]):
                                raise RuntimeError("compiled oracle disagrees with pinned regression " + fixture["name"])
                            compare(area, fixture["case"], live)
                    if selected is None or "threshold" in selected:
                        calibration_queries = 0
                        for total in [1,10**9,10**18,2**64-1]:
                            for stake in sorted({0,1,total//10,total//2,total-1,total}):
                                def query(n): return dict(op="threshold",stake=str(stake),total=str(total),output=n.to_bytes(32,"little").hex())
                                lo, hi = 0,2**253-1
                                while lo < hi:
                                    mid = (lo+hi+1)//2
                                    calibration_queries += 1
                                    if ocaml.query(query(mid)): lo=mid
                                    else: hi=mid-1
                                for n in sorted({max(0,lo-1),lo,min(2**253-1,lo+1)}): compare("threshold",query(n))
                        report["networks"][network]["threshold_calibration_queries"] = calibration_queries
                    if args.corpus and (selected is None or "selection_recorded" in selected):
                        for area,case in recorded(args.corpus,network): compare(area,case)
                    if args.forks and (selected is None or "selection_recorded_forks" in selected):
                        for area,case in recorded_forks(args.forks,network): compare(area,case)
                    if args.rust_trace and (selected is None or "epoch_traces" in selected):
                        tracer = Worker([str(args.rust_trace.resolve()),network],output,network,"rust-trace"); workers.append(tracer)
                        report["networks"][network]["rust_trace_build"] = tracer.query({"op":"build_info"})
                        template, h1, _ = base_state()
                        builder = traces.Builder(template, h1, at_slot, ocaml)
                        for item in traces.traces(builder):
                            case = item["case"]
                            expectations = traces.expected(ocaml, case)
                            try:
                                observed = tracer.query(case)
                            except Exception as error:
                                log.write(json.dumps(dict(area="epoch_traces",trace=item["name"],case=case,adapter_error=str(error)))+"\n")
                                log.flush()
                                raise
                            if len(observed) != len(expectations):
                                raise RuntimeError("trace step count differs: " + item["name"])
                            for index,(seen,expect) in enumerate(zip(observed,expectations)):
                                rust_step = traces.rust_view(seen, expect)
                                equal = canonical(rust_step) == canonical(expect)
                                counts["epoch_traces"] += 1
                                record = dict(area="epoch_traces",trace=item["name"],step=index,
                                    case=case["steps"][index],rust=rust_step,ocaml=expect,equal=equal)
                                log.write(json.dumps(record,separators=(",",":"))+"\n")
                                if not equal:
                                    failures.append(dict(network=network, **record))
                print(network,dict(counts),"mismatches",sum(f["network"]==network for f in failures),flush=True)
            finally:
                exits = [w.close() for w in workers]
                report.setdefault("worker_exits",{})[network] = exits
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        report["elapsed_seconds"] = time.monotonic()-started
        report["mismatches"] = len(failures)
        report["passed"] = not failures and "error" not in report and all(x==0 for xs in report.get("worker_exits",{}).values() for x in xs)
        (output / "report.json").write_text(json.dumps(report,indent=2)+"\n")
        (output / "divergences.json").write_text(json.dumps(failures,indent=2)+"\n")
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
