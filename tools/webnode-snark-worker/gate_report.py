#!/usr/bin/env python3
"""P5.3: summarise the browser SNARK worker gate from its run directory.

OCaml's side is P5.2's `worker_report.py` reading (its sampled best chains
joined into the final canonical chain, the work each block bought from our
worker's key, OCaml's snark verification failures in its logs). The worker's
side is the browser's: each `SnarkWorkerWorkProved` the web node logged (the
bundle's time in the isolated prover, each job's, and the node's self-check),
self-check failures, and the page's memory and the browser's from
`browser/result.json`. If `scan-states.jsonl` (scan_sampler.py) is there, the
work ids each block holds, from the relay's scan state summary, are matched
to the browser's bundles: which kinds of jobs the blocks bought, and that no
block holds work the browser did not prove. Writes
`browser-worker-results.json` and prints it.

    python3 -I tools/webnode-snark-worker/gate_report.py --dir RUN
"""

import argparse
import importlib.util
import json
from pathlib import Path
import re
import statistics

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location(
    "worker_report", HERE.parents[1] / "tools/local-mesa-net/worker_report.py")
worker_report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker_report)

MIB = 1024 * 1024


def stats(values):
    if not values:
        return None
    return {"count": len(values), "min_s": round(min(values) / 1000, 2),
            "median_s": round(statistics.median(values) / 1000, 2),
            "mean_s": round(statistics.mean(values) / 1000, 2),
            "max_s": round(max(values) / 1000, 2)}


def browser_work(console):
    """The web node's multi-line `SnarkWorkerWorkProved` entries."""
    entries, failures, current = [], 0, None
    for line in console.open(errors="replace"):
        if 'kind = "SnarkWorkerWorkProved"' in line:
            current = {}
            continue
        if 'kind = "SnarkWorkerSelfCheckFailed"' in line:
            failures += 1
        if current is None:
            continue
        match = re.match(r'^(\w+) = (.*?);', line)
        if not match:
            continue
        key, value = match.groups()
        current[key] = value.strip('"')
        if key == "queued_ms":
            entries.append({
                "job_id": current.get("job_id"),
                "jobs": current.get("jobs"),
                "prove_ms": int(current.get("prove_ms", 0)),
                "per_job_ms": [int(x) for x in current.get("per_job_ms", "").split(",") if x],
                "check_ms": int(current.get("check_ms", 0)),
                "queued_ms": int(current.get("queued_ms", 0)),
            })
            current = None
    return entries, failures


def blocks_by_work_ids(path, proved, chain):
    """The bundles each sampled block holds, by the browser's job kinds."""
    if not path.exists():
        return None
    kinds = {item["job_id"]: item["jobs"] for item in proved}
    hashes = {int(block["protocolState"]["consensusState"]["blockHeight"]): block["stateHash"]
              for block in chain}
    blocks, bought = [], {}
    for line in path.open():
        if not line.strip():
            continue
        sample = json.loads(line)
        found = [kinds.get(work, "not proved by the browser") for work in sample["completed_works"]]
        for kind in found:
            bought[kind] = bought.get(kind, 0) + 1
        blocks.append({"height": sample["height"], "state_hash": sample["hash"],
                       "canonical": hashes.get(sample["height"]) == sample["hash"],
                       "bundles": len(found),
                       "kinds": {kind: found.count(kind) for kind in sorted(set(found))}})
    merges = [block for block in blocks if any("merge" in kind for kind in block["kinds"])]
    return {"sampled_blocks": len(blocks), "bundles_by_kind": bought,
            "first_block_with_a_merge": merges[0] if merges else None,
            "blocks_with_merges": len(merges), "blocks": blocks}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    args = parser.parse_args()
    run = args.dir.resolve()
    worker = json.loads((run / "public/worker.json").read_text())["publicKey"]
    samples = [json.loads(line) for line in (run / "worker-samples.jsonl").open() if line.strip()]
    chain, seen = worker_report.canonical_chain(samples)
    rows = []
    for block in chain:
        state = block["protocolState"]
        jobs = block.get("snarkJobs", [])
        ours = [job for job in jobs if job["prover"] == worker]
        rows.append({
            "height": int(state["consensusState"]["blockHeight"]),
            "state_hash": block["stateHash"],
            "payments": len(block["transactions"]["userCommands"]),
            "bundles": len(jobs), "our_bundles": len(ours),
            "our_fees_nanomina": sum(int(job["fee"]) for job in ours),
            "other_provers": sorted({job["prover"] for job in jobs} - {worker}),
        })
    proved, failures = browser_work(run / "browser/console.log")
    per_job = {"base": [], "merge": []}
    for item in proved:
        for kind, ms in zip(item["jobs"].split("+"), item["per_job_ms"]):
            per_job[kind].append(ms)
    by_kinds = {}
    for item in proved:
        by_kinds.setdefault(item["jobs"], []).append(item["prove_ms"])
    page = json.loads((run / "browser/result.json").read_text())
    ocaml_counts, ocaml_examples, ocaml_logs = worker_report.ocaml_snark_events(run)
    peak_prover = max((page.get("peak", {}).get("prover") or {"0": 0}).values())
    hosts = [(e.get("type"), e.get("generation"), e.get("at")) for e in page.get("events", [])
             if str(e.get("type", "")).startswith("host-")]
    loads = [s["load"] for s in page.get("browserMemory", {}).get("samples", []) if "load" in s]
    bought = [row for row in rows if row["our_bundles"]]
    by_ids = blocks_by_work_ids(run / "scan-states.jsonl", proved, chain)
    result = {
        "worker_public_key": worker,
        "canonical_blocks": len(chain),
        "first_height": rows[0]["height"] if rows else None,
        "last_height": rows[-1]["height"] if rows else None,
        "blocks_buying_our_work": len(bought),
        "our_bundles_bought": sum(row["our_bundles"] for row in rows),
        "bundles_bought_from_others": sum(row["bundles"] - row["our_bundles"] for row in rows),
        "our_fees_mina": sum(row["our_fees_nanomina"] for row in rows) / 1e9,
        "first_block_with_our_work": bought[0] if bought else None,
        "browser_bundles_proved_and_checked": len(proved),
        "browser_self_check_failures": failures,
        "bundle_prove": {kinds: stats(values) for kinds, values in sorted(by_kinds.items())},
        "per_job_prove": {kind: stats(values) for kind, values in per_job.items()},
        "self_check": stats([item["check_ms"] for item in proved]),
        "queued": stats([item["queued_ms"] for item in proved]),
        "ocaml_snark_events": ocaml_counts, "ocaml_event_examples": ocaml_examples,
        "ocaml_logs_read": ocaml_logs,
        "prover_hosts": hosts,
        "peak_node_linear_mib": round(page.get("peak", {}).get("node", 0) / MIB, 1),
        "peak_prover_linear_mib": round(peak_prover / MIB, 1),
        "browser_peak_rss_mib": round(page.get("browserMemory", {}).get("peak", {}).get("rssKiB", 0) / 1024, 1),
        "browser_peak_pss_mib": round(page.get("browserMemory", {}).get("peak", {}).get("pssKiB", 0) / 1024, 1),
        "load_1min": ({"min": min(loads), "median": statistics.median(loads), "max": max(loads)}
                      if loads else None),
        "page_error": page.get("error") or page.get("pageError"),
        "wasm_sha256": page.get("artifacts", {}).get("wasmSha256"),
        "wasm_commit": (page.get("buildEnv") or {}).get("git", {}).get("commit_hash"),
        "blocks_by_work_ids": by_ids,
    }
    (run / "browser-worker-results.json").write_text(json.dumps(result, indent=2) + "\n")
    with (run / "browser-worker-blocks.jsonl").open("w") as output:
        for row in rows:
            output.write(json.dumps(row) + "\n")
    with (run / "browser-worker-proofs.jsonl").open("w") as output:
        for item in proved:
            output.write(json.dumps(item) + "\n")
    summary = {k: v for k, v in result.items() if k not in ("ocaml_event_examples", "blocks_by_work_ids")}
    if by_ids:
        summary["blocks_by_work_ids"] = {k: v for k, v in by_ids.items() if k != "blocks"}
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
