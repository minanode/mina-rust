#!/usr/bin/env python3
"""Capture unmodified OCaml 4.0.2 reporting on an isolated private test chain.

Run from this repository. Generates NEW THROWAWAY keys; never accepts key paths.
All reports go to the in-process loopback receiver. No producer is configured.
"""

import argparse
import base64
import datetime
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import threading
import time

from receiver import make_server

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True, help="new private run directory")
    parser.add_argument("--network", choices=("mainnet", "devnet"), default="mainnet")
    parser.add_argument("--simplified", action="store_true")
    parser.add_argument("--seconds", type=int, default=90)
    parser.add_argument("--http-port", type=int, default=3590)
    parser.add_argument("--p2p-port", type=int, default=8790)
    parser.add_argument("--receiver-port", type=int, default=3592)
    parser.add_argument("--statuses", default="200", help="per-path response sequence")
    parser.add_argument("--send-commit", action="store_true")
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(mode=0o700)
    for name in ("tmp", "genesis", "keys"):
        (work / name).mkdir(mode=0o700)
    env = dict(os.environ)
    for name in ("MINA_BP_PRIVKEY", "MINA_PRIVATE_KEY", "MINA_CONFIG_FILE"):
        env.pop(name, None)
    env.update(TMPDIR=str(work / "tmp"), MINA_PROFILE=args.network,
               MINA_PRIVKEY_PASS="R43e1-THROWAWAY-TEST-ONLY",
               UPTIME_PRIVKEY_PASS="R43e1-THROWAWAY-TEST-ONLY",
               MINA_LIBP2P_PASS="R43e1-THROWAWAY-TEST-ONLY")
    ocaml = str(ROOT / "scripts/ocaml-mina")
    with (work / "key-generation.log").open("w") as log:
        subprocess.run([ocaml, "advanced", "generate-keypair", "--privkey-path",
                        str(work / "keys/submitter")], env=env, stdout=log,
                       stderr=subprocess.STDOUT, check=True)
        subprocess.run([ocaml, "libp2p", "generate-keypair", "--privkey-path",
                        str(work / "keys/p2p")], env=env, stdout=log,
                       stderr=subprocess.STDOUT, check=True)
    # Only freshly generated test key material is ever dumped, into a private file.
    with (work / "THROWAWAY-keypair.txt").open("w") as stream:
        subprocess.run([ocaml, "advanced", "dump-keypair", "--privkey-path",
                        str(work / "keys/submitter")], env=env, stdout=stream,
                       stderr=subprocess.DEVNULL, check=True)
    public_key = (work / "keys/submitter.pub").read_text().strip()
    config = {
        "genesis": {"genesis_state_timestamp": "2026-10-21T00:00:00Z"},
        "proof": {"level": "none", "block_window_duration_ms": 2000},
        "ledger": {"accounts": [{"pk": public_key, "balance": "1000"}]},
        "daemon": {"network_id": "r43e1-offline-fixture-only"},
    }
    (work / "daemon.json").write_text(json.dumps(config, indent=2) + "\n")
    records = work / "requests.jsonl"
    server = make_server(args.receiver_port, records, tuple(map(int, args.statuses.split(","))))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}"
    # This private chain has no public peers and consumes no mainnet RPC budget.
    command = [ocaml, "daemon",
               "--config-directory", str(work / "conf"),
               "--config-file", str(work / "daemon.json"),
               "--genesis-ledger-dir", str(work / "genesis"),
               "--libp2p-keypair", str(work / "keys/p2p"),
               "--seed", "--isolate-network", "true", "--proof-level", "none",
               "--external-ip", "127.0.0.1", "--bind-ip", "127.0.0.1",
               "--external-port", str(args.p2p_port),
               "--client-port", str(args.p2p_port + 1),
               "--rest-port", str(args.http_port),
               "--enable-peer-exchange", "false",
               "--uptime-url", url + "/uptime", "--uptime-submitter-key",
               str(work / "keys/submitter"),
               "--node-status-url", url + "/node-status",
               "--node-error-url", url + "/node-error",
               "--simplified-node-stats", str(args.simplified).lower(),
               "--log-level", "Info", "--file-log-level", "Info"]
    if args.send_commit:
        command.append("--uptime-send-node-commit-sha")
    start = time.monotonic()
    with (work / "daemon.log").open("w") as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        (work / "daemon.pid").write_text(str(process.pid) + "\n")
        try:
            while time.monotonic() - start < args.seconds and process.poll() is None:
                time.sleep(1)
                if records.exists():
                    paths = [record["path"] for line in records.read_text().splitlines()
                             if (record := json.loads(line))["response_status"] == 200]
                    if "/uptime" in paths and "/node-status" in paths:
                        break
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            server.shutdown()
            server.server_close()
            thread.join()
    binary = Path(os.environ.get("OCAML_MINA_PREFIX", str(Path.home() / ".local/opt/mina-4.0.2"))) / "usr/local/bin/mina"
    manifest = {"network": args.network, "simplified": args.simplified,
                "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "source_commit": "84b8d96954b218f372b93f65a071abf528e6bb61",
                "private_chain": True, "proof_level": "none", "slot_ms": 2000,
                "elapsed_seconds": round(time.monotonic() - start, 3),
                "exit_code": process.returncode,
                "response_statuses": args.statuses, "send_commit": args.send_commit,
                "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}
    (work / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    counts = {}
    if records.exists():
        for line in records.read_text().splitlines():
            record = json.loads(line)
            path = record["path"].strip("/")
            counts[path] = counts.get(path, 0) + 1
            (work / f"{path}-{counts[path]}.json").write_bytes(base64.b64decode(record["body_base64"]))
    print(json.dumps({"work": str(work), "counts": counts, "exit_code": process.returncode}))
    if not counts.get("uptime") or not counts.get("node-status"):
        raise SystemExit("Expected reports not captured; inspect private daemon.log")


if __name__ == "__main__":
    main()
