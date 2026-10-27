#!/usr/bin/env python3
"""Join production attempts, proof checks and OCaml best-chain observations."""

import argparse
import json
from pathlib import Path
import re


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    args = parser.parse_args()
    work = args.dir
    attempts = {}
    accepted = {}
    commands = {}
    last = {}
    with (work / "samples.jsonl").open() as samples:
        for line in samples:
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue  # A concurrent writer may have an incomplete final line.
            last = row
            producer = row.get("producer") or {}
            for attempt in producer.get("attempts", []):
                slot = attempt["won_slot"]["global_slot"]
                state_hash = (attempt.get("block") or {}).get("hash")
                if state_hash is not None:
                    attempts.pop((slot, None), None)
                attempts[(slot, state_hash)] = attempt
            for block in ((row.get("ocaml") or {}).get("data") or {}).get("bestChain") or []:
                accepted.setdefault(block["stateHash"], row["time"])
                commands[block["stateHash"]] = [command["hash"] for command in
                                                block["transactions"]["userCommands"]]
    verified = set()
    injected = {}
    log = work / "logs/rust-netns.log"
    if log.exists():
        with log.open() as lines:
            for line in lines:
                if "Block proof verified in prover service" in line:
                    match = re.search(r'block_hash: "([^"]+)"', line)
                    if match:
                        verified.add(match.group(1))
                if 'kind: "BlockProducerBlockInjected"' in line:
                    match = re.search(r'block_hash: "([^"]+)"', line)
                    if match:
                        injected[match.group(1)] = line.split()[0]
    frontier = {}
    received = {}
    rejections = []
    ocaml_log = work / "ocaml-netns/mina.log"
    if ocaml_log.exists():
        with ocaml_log.open() as lines:
            for line in lines:
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if event.get("message") == "Saw block with state hash $state_hash":
                    frontier.setdefault(event["metadata"]["state_hash"], event["timestamp"])
                if event.get("message") == "Received a block from $sender":
                    received.setdefault(event["metadata"]["state_hash"], []).append(event["timestamp"])
                if "was rejected for reason" in event.get("message", ""):
                    rejections.append(event)
    rows = []
    for (slot, _hash), attempt in sorted(attempts.items(), key=lambda item: (item[0][0], item[0][1] or "")):
        block = attempt.get("block") or {}
        times = attempt["times"]
        start, end = times.get("proof_create_start"), times.get("proof_create_end")
        state_hash = block.get("hash")
        rows.append({
            "slot": slot, "height": block.get("height"), "hash": state_hash,
            "status": attempt["status"],
            "proof_seconds_including_self_check": (end - start) / 1e9 if start and end else None,
            "service_self_verified": state_hash in verified,
            "injected_after_state_machine_verification": injected.get(state_hash),
            "ocaml_received_log": next(iter(received.get(state_hash, [])), None),
            "ocaml_receipt_times": received.get(state_hash, []),
            "ocaml_rejections": [event for event in rejections
                                 if event["metadata"]["state_hash"] == state_hash],
            "ocaml_frontier_log": frontier.get(state_hash),
            "ocaml_best_chain_first_seen": accepted.get(state_hash),
            "ocaml_user_command_hashes": commands.get(state_hash),
            "transactions": block.get("transactions"),
            "completed_works_count": block.get("completed_works_count"),
            "coinbase_nanomina": block.get("coinbase"),
            "fees_nanomina": block.get("fees"),
            "discard_reason": attempt.get("discard_reason"),
        })
    report = {
        "last_sample": last.get("time"),
        "rust_block_candidates": sum(row["hash"] is not None for row in rows),
        "rust_blocks": sum(row["injected_after_state_machine_verification"] is not None for row in rows),
        "ocaml_accepted_rust_blocks": sum(row["ocaml_best_chain_first_seen"] is not None for row in rows),
        "self_verified_rust_blocks": sum(row["service_self_verified"] for row in rows),
        "ocaml_rejections": rejections,
        "blocks": rows,
    }
    (work / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
