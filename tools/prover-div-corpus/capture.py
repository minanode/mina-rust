#!/usr/bin/env python3
"""Capture public job specifications from a task-owned observer, read-only."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import time
import urllib.parse
import urllib.request


def get(endpoint, path, binary=False):
    request = urllib.request.Request(endpoint + path)
    if binary:
        request.add_header("Accept", "application/octet-stream")
    with urllib.request.urlopen(request, timeout=30) as response:
        data = response.read(64 * 1024 * 1024 + 1)
    if len(data) > 64 * 1024 * 1024:
        raise ValueError("response exceeds capture limit")
    return data if binary else json.loads(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--bundles", type=int, default=60)
    parser.add_argument("--seconds", type=int, default=1200)
    args = parser.parse_args()
    url = urllib.parse.urlparse(args.endpoint)
    if url.scheme != "http" or url.hostname not in ("localhost", "127.0.0.1", "::1"):
        parser.error("capture requires the loopback endpoint of your own observer")
    args.output.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + args.seconds
    seen = set()
    while time.monotonic() < deadline and len(seen) < args.bundles:
        if shutil.disk_usage(args.output).free < 56_000_000_000:
            raise SystemExit("capture stopped: less than 56 GB free")
        try:
            status = get(args.endpoint, "/status")
            frontier = status["transition_frontier"]
            if frontier["sync"]["status"] != "Synced":
                print("waiting:", frontier["sync"]["status"], flush=True)
                time.sleep(15)
                continue
            jobs = get(args.endpoint, "/snark-pool/jobs")
            # These filters contain only public protocol states, never node config.
            needed = get(args.endpoint, "/state?filter=" + urllib.parse.quote("$.transition_frontier.needed_protocol_states"))
            headers = get(args.endpoint, "/state?filter=" + urllib.parse.quote("$.transition_frontier.best_chain[*].block.block.header.protocol_state"))
            states = list(needed.values()) + headers
            for job in jobs:
                job_id = job["id"]
                if job_id in seen:
                    continue
                full = get(args.endpoint, "/snark-pool/job/" + urllib.parse.quote(job_id))
                # Hash every state in Rust before associating it with a base job.
                raw = json.dumps({"job": full["job"], "protocol_states": states}, sort_keys=True).encode()
                digest = hashlib.sha256(raw).hexdigest()
                (args.output / (digest + ".capture")).write_bytes(raw)
                metadata = {"network": status["network"], "chain_id": status["chain_id"],
                            "tip": frontier["best_tip"], "bundle_id": job_id,
                            "captured_at_unix": time.time(), "sha256": digest}
                (args.output / (digest + ".json")).write_text(json.dumps(metadata, indent=2) + "\n")
                seen.add(job_id)
                print("captured", len(seen), digest, flush=True)
                if len(seen) >= args.bundles:
                    break
        except (OSError, ValueError, KeyError) as error:
            print("capture retry:", type(error).__name__, str(error), flush=True)
        if len(seen) < args.bundles:
            time.sleep(15)
    print("captured bundles:", len(seen), flush=True)
    if not seen:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
