#!/usr/bin/env python3
"""P5.2: summarise a SNARK worker gate from its run directory.

Joins OCaml's sampled best chains into its final canonical chain (parent links
from the last tip), counts the work each block bought from our worker's key,
finds the blocks that emitted a ledger proof and moved the snarked ledger, and
reads the Rust worker's per-job proof times and self-checks from its log and
OCaml's snark verification failures from its log. P5.2e: the zkApp commands
sent and the blocks that included them; the bundles bought in the blocks our
node archived (rust-netns/archive-precomputed), tied to our worker's proofs by
job ID, and so the zkApp bundles we proved and OCaml bought; the zkApp
provers' load cost and the node's peak memory. Writes worker-results.json and worker-blocks.csv (and, with zkApp
commands, zkapp-results.json); reads only local files.
"""

import argparse
import csv
import json
from pathlib import Path
import re
import statistics


def canonical_chain(samples):
    blocks, tip = {}, None
    for row in samples:
        chain = ((row.get("ocaml") or {}).get("data") or {}).get("bestChain") or []
        for block in chain:
            blocks[block["stateHash"]] = block
        if chain:
            tip = chain[-1]["stateHash"]
    chain = []
    while tip in blocks:
        chain.append(blocks[tip])
        tip = blocks[tip]["protocolState"]["previousStateHash"]
    return list(reversed(chain)), blocks


def rust_events(log_path):
    """The worker's prover loads, failed jobs and zkApp job counts."""
    loads, failures, zkapp_jobs = [], [], 0
    load = re.compile(r'^(\S+).*kind: "(SnarkWorker(?:Zkapp)?ProversLoaded)".*?load_ms: (\d+)'
                      r'.*?rss_before_kib: (\d+).*?rss_after_kib: (\d+)')
    for line in log_path.open(errors="replace"):
        if "ProversLoaded" in line:
            match = load.search(line)
            if match:
                time, kind, ms, before, after = match.groups()
                loads.append({"time": time, "kind": kind, "load_ms": int(ms),
                              "rss_before_kib": int(before), "rss_after_kib": int(after)})
        elif "SnarkWorkerWorkFailed" in line:
            failures.append(line.strip()[:600])
        elif "SnarkWorkerZkappJobs" in line:
            zkapp_jobs += 1
    return loads, failures, zkapp_jobs


def zkapp_report(work, chain, proved, worker):
    """Each zkApp command sent: its block, its job's bundle, our proof of it
    and the canonical block that bought it."""
    sent = [json.loads(line) for line in (work / "zkapps.jsonl").open() if line.strip()]
    commands = {row["hash"]: {"nonce": row["nonce"], "submitted": row["time"],
                              "reply": row["reply"]}
                for row in sent if "hash" in row}
    summaries = [json.loads(line) for line in (work / "scan-summaries.jsonl").open()
                 if line.strip()] if (work / "scan-summaries.jsonl").exists() else []
    canonical = {block["stateHash"]: block for block in chain}
    height_of = {block["stateHash"]: int(block["protocolState"]["consensusState"]["blockHeight"])
                 for block in chain}
    bundles = {}
    for summary in summaries:
        for job in summary["zkapp_jobs"]:
            if job.get("hash") in commands:
                bundles.setdefault(job["hash"], set()).add(job["bundle_job_id"])
    ours = {item["job_id"]: item for item in proved}
    rows = []
    for command_hash, command in commands.items():
        included = [(height_of[h], h, z.get("failureReason"))
                    for h, block in canonical.items()
                    for z in block["transactions"].get("zkappCommands") or []
                    if z["hash"] == command_hash]
        row = {"hash": command_hash, **command,
               "included": [{"height": height, "state_hash": h, "failure_reason": reason}
                            for height, h, reason in included],
               "bundle_job_ids": sorted(bundles.get(command_hash, ()))}
        row["our_proofs"] = [ours[job] for job in row["bundle_job_ids"] if job in ours]
        bought = []
        for summary in summaries:
            if summary["ocaml_hash"] not in canonical:
                continue
            for job in row["bundle_job_ids"]:
                if job in (summary.get("completed_works") or []):
                    block = canonical[summary["ocaml_hash"]]
                    bought.append({"height": summary["height"], "state_hash": summary["ocaml_hash"],
                                   "bundle_job_id": job,
                                   "block_provers": sorted({j["prover"] for j in block.get("snarkJobs", [])}),
                                   "our_key_paid": sum(int(j["fee"]) for j in block.get("snarkJobs", [])
                                                       if j["prover"] == worker)})
        row["bought_in"] = bought
        rows.append(row)
    return rows


def archived_purchases(work, proved, worker):
    """Every bundle bought in the blocks our node archived, with our proof of
    it: the job ID is the bundle's first source and last target ledgers, as
    the worker logs it."""
    ours = {item["job_id"]: item for item in proved}

    def statements(proofs):
        if isinstance(proofs, dict):
            return [proofs["statement"]]
        if isinstance(proofs, list):
            return [s for item in proofs if item not in ("One", "Two") for s in statements(item)]
        return []

    def passes(side):
        return f'{side["first_pass_ledger"]}_{side["second_pass_ledger"]}'

    rows = []
    for path in sorted((work / "rust-netns/archive-precomputed").glob("*.json")):
        # Sok digests are raw bytes in these files; the fields read are ASCII.
        block = json.loads(path.read_text(encoding="latin-1"))
        block = block.get("data", block)
        height = int(block["protocol_state"]["body"]["consensus_state"]["blockchain_length"])
        for part in block["staged_ledger_diff"]["diff"]:
            for work_item in (part or {}).get("completed_works", []):
                bundle = statements(work_item["proofs"])
                job_id = f'{passes(bundle[0]["source"])}-{passes(bundle[-1]["target"])}'
                proof = ours.get(job_id)
                rows.append({"height": height, "state_hash": path.stem.split("-", 2)[2],
                             "prover": work_item["prover"], "fee": work_item["fee"],
                             "job_id": job_id, "ours": work_item["prover"] == worker,
                             "our_proof": proof,
                             "zkapp": bool(proof and "zkapp" in proof["jobs"])})
    return rows


def rust_work(log_path):
    proved, failed, rejected, skipped = [], 0, [], []
    pattern = re.compile(r'^(\S+).*kind: "SnarkWorkerWorkProved".*?job_id: (\S+?),.*?jobs: "([^"]+)".*?'
                         r'prove_ms: (\d+).*?per_job_ms: "([^"]*)".*?check_ms: (\d+)')
    for line in log_path.open(errors="replace"):
        if "SnarkWorkerWorkProved" in line:
            match = pattern.search(line)
            if match:
                time, job, kinds, prove, per_job, check = match.groups()
                proved.append({"time": time, "job_id": job, "jobs": kinds,
                               "prove_ms": int(prove), "check_ms": int(check),
                               "per_job_ms": [int(x) for x in per_job.split(",") if x]})
        elif "SnarkWorkerSelfCheckFailed" in line:
            failed += 1
        elif "SnarkPoolAdmissionRejected" in line and "Verified" in line:
            rejected.append(line.strip()[:400])
        elif "SnarkWorkerZkappJobsSkipped" in line:  # P5.2's runs
            skipped.append(line.strip()[:200])
    return proved, failed, rejected, skipped


def stats(values):
    if not values:
        return None
    return {"count": len(values), "min_s": min(values) / 1000, "median_s": statistics.median(values) / 1000,
            "max_s": max(values) / 1000, "mean_s": round(statistics.mean(values) / 1000, 3)}


def ocaml_snark_events(work):
    counts = {"verify_errors": 0, "invalid_proof_trust": 0, "rejected_not_referenced": 0,
              "insufficient_fee": 0}
    examples = []
    logs = sorted((work / "ocaml-netns").glob("mina.log*"))
    for path in logs:
        for line in path.open(errors="replace"):
            if "Error verifying transaction snark" in line:
                counts["verify_errors"] += 1
                examples.append(line[:500])
            elif "Sent_invalid_proof" in line:
                counts["invalid_proof_trust"] += 1
                examples.append(line[:500])
            elif "statement not referenced" in line:
                counts["rejected_not_referenced"] += 1
            elif "sufficient fee to create prover" in line:
                counts["insufficient_fee"] += 1
    return counts, examples[:10], [str(p.name) for p in logs]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--worker", help="the worker's public key (default: keys/small.pub)")
    args = parser.parse_args()
    work = args.dir.resolve()
    worker = args.worker or (work / "keys/small.pub").read_text().strip()
    samples = [json.loads(line) for line in (work / "worker-samples.jsonl").open() if line.strip()]
    chain, seen = canonical_chain(samples)
    genesis_snarked = None
    rows = []
    for block in chain:
        state = block["protocolState"]
        snarked = state["blockchainState"]["snarkedLedgerHash"]
        genesis_snarked = genesis_snarked or snarked
        jobs = block.get("snarkJobs", [])
        ours = [job for job in jobs if job["prover"] == worker]
        rows.append({
            "height": int(state["consensusState"]["blockHeight"]),
            "slot_since_genesis": int(state["consensusState"]["slotSinceGenesis"]),
            "state_hash": block["stateHash"],
            "payments": len(block["transactions"]["userCommands"]),
            "coinbase": block["transactions"]["coinbase"],
            "bundles": len(jobs), "our_bundles": len(ours),
            "our_fees_nanomina": sum(int(job["fee"]) for job in ours),
            "other_provers": sorted({job["prover"] for job in jobs} - {worker}),
            "proof_emitted": bool(state["blockchainState"].get("stagedLedgerProofEmitted")),
            "snarked_ledger": snarked,
        })
    # The first sampled block's parent may already be past genesis; the
    # emitted-proof flag and a change of snarked ledger are what count.
    advances = [row for i, row in enumerate(rows) if i and row["snarked_ledger"] != rows[i - 1]["snarked_ledger"]]
    proved, failed, rejected, skipped = rust_work(work / "logs/rust-netns.log")
    per_job = {"base": [], "merge": []}
    for item in proved:
        for kind, ms in zip(item["jobs"].split("+"), item["per_job_ms"]):
            per_job.setdefault(kind, []).append(ms)
    loads, work_failures, zkapp_job_logs = rust_events(work / "logs/rust-netns.log")
    samples_memory = [row.get("rust_memory_kib") or {} for row in samples]
    peak_kib = max((m.get("VmHWM", 0) for m in samples_memory), default=0)
    zkapps = zkapp_report(work, chain, proved, worker) if (work / "zkapps.jsonl").exists() else []
    archived = archived_purchases(work, proved, worker)
    payments = [json.loads(line) for line in (work / "payments.jsonl").open() if line.strip()] \
        if (work / "payments.jsonl").exists() else []
    submitted = [row for row in payments if "nonce" in row]
    accepted = [row for row in submitted
                if ((row.get("ocaml") or {}).get("data") or {}).get("sendPayment")]
    ocaml_counts, ocaml_examples, ocaml_logs = ocaml_snark_events(work)
    result = {
        "worker_public_key": worker,
        "canonical_blocks": len(chain),
        "blocks_seen": len(seen),
        "first_height": rows[0]["height"] if rows else None,
        "last_height": rows[-1]["height"] if rows else None,
        "blocks_buying_our_work": sum(1 for row in rows if row["our_bundles"]),
        "our_bundles_bought": sum(row["our_bundles"] for row in rows),
        "bundles_bought_from_others": sum(row["bundles"] - row["our_bundles"] for row in rows),
        "our_fees_mina": sum(row["our_fees_nanomina"] for row in rows) / 1e9,
        "proof_emitted_blocks": [row for row in rows if row["proof_emitted"]],
        "snarked_ledger_advances": advances,
        "payments_submitted": len(submitted), "payments_accepted_by_ocaml": len(accepted),
        "payments_in_canonical_blocks": sum(row["payments"] for row in rows),
        "rust_bundles_proved_and_checked": len(proved),
        "rust_self_check_failures": failed,
        "rust_own_work_rejected_after_verification": rejected[:10],
        "rust_zkapp_skip_logs": len(skipped),
        "bundle_prove_ms": stats([item["prove_ms"] for item in proved]),
        "bundle_check_ms": stats([item["check_ms"] for item in proved]),
        "per_job_prove": {kind: stats(values) for kind, values in per_job.items()},
        "ocaml_snark_events": ocaml_counts, "ocaml_event_examples": ocaml_examples,
        "ocaml_logs_read": ocaml_logs,
        "first_proved": proved[0]["time"] if proved else None,
        "last_proved": proved[-1]["time"] if proved else None,
        "prover_loads": loads,
        "rust_work_failures": work_failures[:20],
        "rust_work_failure_count": len(work_failures),
        "rust_zkapp_job_logs": zkapp_job_logs,
        "rust_peak_rss_kib": peak_kib,
        "zkapp_commands_sent": len(zkapps),
        "zkapp_commands_included": sum(1 for row in zkapps if row["included"]),
        "zkapp_bundles_proved": sum(1 for item in proved if "zkapp" in item["jobs"]),
        "archived_bundles_bought": len(archived),
        "archived_bundles_bought_ours_with_our_proof": sum(1 for row in archived if row["ours"] and row["our_proof"]),
        "zkapp_bundles_bought": [row for row in archived if row["zkapp"]],
    }
    if zkapps:
        (work / "zkapp-results.json").write_text(json.dumps(zkapps, indent=2) + "\n")
    (work / "worker-results.json").write_text(json.dumps(result, indent=2) + "\n")
    with (work / "worker-blocks.csv").open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]) if rows else ["height"], lineterminator="\n")
        writer.writeheader()
        for row in rows:
            writer.writerow({**row, "other_provers": " ".join(row["other_provers"])})
    with (work / "worker-proofs.jsonl").open("w") as output:
        for item in proved:
            output.write(json.dumps(item) + "\n")
    summary = {key: value for key, value in result.items()
               if key not in ("proof_emitted_blocks", "snarked_ledger_advances", "ocaml_event_examples")}
    summary["proof_emitted_heights"] = [row["height"] for row in result["proof_emitted_blocks"]]
    summary["snarked_advance_heights"] = [row["height"] for row in advances]
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
