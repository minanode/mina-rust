#!/usr/bin/env python3
"""P5.2: sample an OCaml producer and a Rust SNARK worker in the namespace.

Every interval: OCaml's recent best chain (completed work bought per block,
with prover and fee, its zkApp commands, and whether the block emitted a
ledger proof), OCaml's snark pool, and the Rust worker's status, worker
state and memory (P5.2e: resident and peak, from /proc). For each new block
height, our node's scan-state summary of that block: the work it bought
(job IDs) and the zkApp command jobs in its scan state, to tie a bought
bundle to the zkApp job it proves. Only this run's own node APIs are queried.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import time
import urllib.request

from processes import track


QUERY = """{ syncStatus daemonStatus { blockchainLength chainId }
  bestChain(maxLength: 30) { stateHash protocolState {
    previousStateHash
    blockchainState { snarkedLedgerHash stagedLedgerHash stagedLedgerProofEmitted
      ledgerProofStatement { sourceRegisters { firstPassLedger secondPassLedger }
        targetRegisters { firstPassLedger secondPassLedger } } }
    consensusState { blockHeight slotSinceGenesis }
  } transactions { userCommands { hash nonce } coinbase
    zkappCommands { hash failureReason { index failures } }
    feeTransfer { recipient fee type } }
    snarkJobs { prover fee workIds } }
  snarkPool { prover fee workIds }
}"""


def fetch(url, query=None):
    try:
        request = urllib.request.Request(url)
        if query is not None:
            request.data = json.dumps({"query": query}).encode()
            request.add_header("Content-Type", "application/json")
        with urllib.request.urlopen(request, timeout=10) as response:
            return json.load(response)
    except Exception as error:
        return {"sample_error": str(error)}


def memory(work):
    """The Rust node's resident and peak memory in KiB, from its status."""
    try:
        pid = int((work / "rust.pid").read_text())
        fields = {}
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            key, _, value = line.partition(":")
            if key in ("VmRSS", "VmHWM"):
                fields[key] = int(value.split()[0])
        return fields
    except Exception as error:
        return {"error": str(error)}


def zkapp_jobs(summary):
    """The block's bought work and transactions, and its zkApp jobs."""
    block = summary.get("block") or {}
    jobs = []
    for level in summary.get("scan_state") or []:
        for job in level:
            kind = job.get("job") if isinstance(job, dict) else None
            base = kind.get("Base") if isinstance(kind, dict) else None
            if base and base.get("kind") == "Zkapp":
                jobs.append({key: job.get(key) for key in
                             ("status", "job_id", "bundle_job_id", "seq_no", "snark")}
                            | {"hash": base.get("hash")})
    return {"hash": block.get("hash"), "height": block.get("height"),
            "completed_works": block.get("completed_works"),
            "transactions": [tx for tx in block.get("transactions") or []
                             if tx.get("kind") == "Zkapp"],
            "zkapp_jobs": jobs}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--rust-port", type=int, default=3660)
    parser.add_argument("--ocaml-port", type=int, default=3670)
    parser.add_argument("--seconds", type=int, default=14400)
    parser.add_argument("--interval", type=int, default=15)
    args = parser.parse_args()
    work = args.dir.resolve()
    assert os.readlink("/proc/self/ns/net") == json.loads((work / "namespace.json").read_text())["netns"]
    (work / "sample.pid").write_text(str(os.getpid()) + "\n")
    end = time.monotonic() + args.seconds
    rust = f"http://127.0.0.1:{args.rust_port}"
    ocaml = f"http://127.0.0.1:{args.ocaml_port}/graphql"
    summarised = set()
    with (work / "worker-samples.jsonl").open("a", buffering=1) as output, \
            (work / "scan-summaries.jsonl").open("a", buffering=1) as summaries:
        while time.monotonic() < end:
            track(work)
            now = datetime.datetime.now(datetime.timezone.utc).isoformat()
            row = {
                "time": now,
                "rust_status": fetch(rust + "/status"),
                "rust_workers": fetch(rust + "/snarker/workers"),
                "rust_memory_kib": memory(work),
                "ocaml": fetch(ocaml, QUERY),
            }
            output.write(json.dumps(row) + "\n")
            chain = ((row["ocaml"].get("data") or {}).get("bestChain") or [])
            for block in chain:
                height = int(block["protocolState"]["consensusState"]["blockHeight"])
                if height in summarised:
                    continue
                summary = fetch(f"{rust}/scan-state/summary/{height}")
                if "sample_error" in summary or "block" not in summary:
                    continue
                summarised.add(height)
                summaries.write(json.dumps({"time": now, "ocaml_hash": block["stateHash"],
                                            **zkapp_jobs(summary)}) + "\n")
            time.sleep(args.interval)


if __name__ == "__main__":
    main()
