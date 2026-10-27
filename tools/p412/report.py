#!/usr/bin/env python3
"""Join browser proof, relay, and OCaml evidence from one P4.12 run."""

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path


def iso(milliseconds):
    return datetime.fromtimestamp(milliseconds / 1000, timezone.utc).isoformat()


def read_json(path):
    return json.loads(path.read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", required=True, type=Path)
    parser.add_argument("--browser", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    run = args.run.resolve()
    browser = read_json(args.browser / "result.json")
    config = read_json(run / "daemon.json")
    producer = config["ledger"]["accounts"][1]["pk"]
    samples = []
    for line in (run / "samples.jsonl").read_text().splitlines():
        try:
            samples.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    assert samples, "no relay/OCaml samples"
    # The browser closes its WebRTC peer after settling. Preserve the last
    # common-chain observation while that peer was still connected.
    last = read_json(run / "final-observation.json") if (
        run / "final-observation.json").exists() else samples[-1]
    received = {}
    frontier = {}
    rejections = []
    events = []
    for line in (run / "ocaml-netns/mina.log").read_text().splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        message = event.get("message", "")
        state_hash = event.get("metadata", {}).get("state_hash")
        if message == "Received a block from $sender" and state_hash:
            received.setdefault(state_hash, event["timestamp"])
            events.append({"timestamp": event["timestamp"], "message": message,
                           "state_hash": state_hash})
        elif message == "Saw block with state hash $state_hash" and state_hash:
            frontier.setdefault(state_hash, event["timestamp"])
            events.append({"timestamp": event["timestamp"], "message": message,
                           "state_hash": state_hash})
        elif "was rejected for reason" in message:
            rejections.append(event)
            events.append({"timestamp": event["timestamp"], "message": message,
                           "state_hash": state_hash,
                           "reason": event.get("metadata", {}).get("reason")})
    first_seen = {}
    for sample in samples:
        chain = ((sample.get("ocaml") or {}).get("data") or {}).get("bestChain") or []
        for block in chain:
            first_seen.setdefault(block["stateHash"], sample["time"])
    proof_events = {}
    for event in browser["events"]:
        if event["type"] == "prove-end":
            proof_events[(event["generation"], event["requestId"])] = event
    proofs = {}
    for item in browser["verification"]:
        parts = item["response"].removesuffix(".bin").split("-")
        event = proof_events.get((int(parts[1]), int(parts[2])))
        report = item["report"] or {}
        proofs[report.get("block_hash")] = {
            "round_trip_seconds": event["ms"] / 1000 if event else None,
            "finished_at": iso(event["at"]) if event else None,
            "native_verification_status": item["status"],
            "native_verification": report,
            "response_sha256": item["sha256"],
        }
    final_chain = ((last.get("ocaml") or {}).get("data") or {}).get("bestChain") or []
    final_hashes = {block["stateHash"] for block in final_chain}
    deadline_errors = list(dict.fromkeys(
        line.split('";')[0].strip() + '"'
        for line in (args.browser / "console.log").read_text().splitlines()
        if 'error = "not enough time before block proof deadline:' in line
    ))
    blocks = []
    discarded = []
    for attempt in browser["attempts"]:
        block = attempt.get("block") or {}
        state_hash = block.get("hash")
        if not state_hash:
            continue
        if attempt["status"] == "Discarded":
            discarded.append({
                "slot": attempt["won_slot"]["global_slot"],
                "height": block["height"], "hash": state_hash,
                "reason": attempt.get("discard_reason"),
                "proof_errors": deadline_errors,
                "ocaml_received_at": received.get(state_hash),
            })
            continue
        assert attempt["status"] in ("BlockApplySuccess", "Committed", "Canonical")
        times = attempt["times"]
        start, end = times.get("proof_create_start"), times.get("proof_create_end")
        blocks.append({
            "producer_origin": "headless Chrome web node",
            "producer": attempt["won_slot"]["delegator"][0],
            "slot": attempt["won_slot"]["global_slot"],
            "height": block["height"], "hash": state_hash,
            "browser_status": attempt["status"],
            "browser_proof": proofs.get(state_hash),
            "browser_proof_and_self_check_seconds": (end - start) / 1e9 if start and end else None,
            "ocaml_received_at": received.get(state_hash),
            "ocaml_frontier_at": frontier.get(state_hash),
            "ocaml_best_chain_first_seen": first_seen.get(state_hash),
            "ocaml_verdict": "adopted" if state_hash in final_hashes and state_hash in frontier else "not adopted",
            "transactions": block.get("transactions"),
            "coinbase_nanomina": block.get("coinbase"),
            "fees_nanomina": block.get("fees"),
        })
    output = {
        "producer": producer,
        "browser_started_at": browser["machine"]["startedAt"],
        "browser_network": browser.get("browserNetwork"),
        "browser_chain_id": browser.get("chainId"),
        "browser_final_tip": (browser.get("finalStatus") or {}).get("transition_frontier", {}).get("best_tip"),
        "relay_final_tip": (last.get("rust_status") or {}).get("transition_frontier", {}).get("best_tip"),
        "ocaml_final_chain": final_chain,
        "last_sample_at": last["time"],
        "genesis_proof": next((proof for proof in proofs.values()
                               if proof["native_verification"].get("is_genesis")), None),
        "blocks": blocks,
        "browser_discarded_attempts": discarded,
        "ocaml_rejections": rejections,
        "ocaml_events": events,
        "browser_final_accounts": browser.get("finalAccounts"),
        "relay_ocaml_final_audit": read_json(run / "audit-final.json") if (run / "audit-final.json").exists() else None,
        "payment_response": read_json(run / "payment-response.json") if (run / "payment-response.json").exists() else None,
    }
    args.out.write_text(json.dumps(output, indent=2) + "\n")
    print(f"Browser blocks: {len(blocks)}; OCaml adopted: "
          f"{sum(block['ocaml_verdict'] == 'adopted' for block in blocks)}")


if __name__ == "__main__":
    main()
