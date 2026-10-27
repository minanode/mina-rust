#!/usr/bin/env python3
"""Run one private-network node inside an already-held lock.sh node slot."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess

from network_profile import load_profile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("role", choices=("rust", "ocaml"))
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    parser.add_argument("--blobs", type=Path, required=True)
    parser.add_argument("--private-isolated-allocation", action="store_true",
                        help="explicit authorization for a route-isolated pair without node locks")
    parser.add_argument("--relay-only", action="store_true",
                        help="native Rust relay for a browser producer; no producer key")
    parser.add_argument("--rust-slot", type=int, default=26)
    parser.add_argument("--ocaml-slot", type=int, default=27)
    parser.add_argument("--ocaml-snark-worker", action="store_true")
    # P5.2: OCaml produces, Rust only sells SNARK work, for the fee key of
    # keys/<name>.pub at a fee in MINA.
    parser.add_argument("--ocaml-producer", action="store_true")
    parser.add_argument("--rust-snark-worker-fee", help="MINA; Rust runs as a SNARK worker")
    parser.add_argument("--snark-worker-key", default="small")
    args = parser.parse_args()
    assert not args.relay_only or args.role == "rust"
    assert not (args.relay_only and args.rust_snark_worker_fee)
    if not args.private_isolated_allocation:
        os.fstat(9)  # lock.sh passes its held flock on descriptor 9.
    os.umask(0o077)
    root = Path(__file__).resolve().parents[2]
    work = args.dir.resolve()
    network = load_profile(work)
    namespace = json.loads((work / "namespace.json").read_text())
    assert os.readlink("/proc/self/ns/net") == namespace["netns"], "use enter.sh before lock.sh"
    if args.private_isolated_allocation:
        interfaces = json.loads(subprocess.check_output(["ip", "-json", "address"], text=True))
        routes = json.loads(subprocess.check_output(["ip", "-json", "route", "show", "table", "all"], text=True))
        routes6 = json.loads(subprocess.check_output(["ip", "-6", "-json", "route", "show", "table", "all"], text=True))
        assert [row["ifname"] for row in interfaces] == ["lo"]
        assert all(row.get("dev") == "lo" and row.get("dst") != "default" for row in routes + routes6)
        (work / f"{args.role}-isolation-preflight.json").write_text(json.dumps({
            "allocation": "authorized route-isolated private pair, no node slots",
            "netns": namespace["netns"], "interfaces": interfaces, "routes": routes,
            "routes6": routes6,
        }, indent=2) + "\n")
    matched = json.loads((work / "identity-match.json").read_text())
    assert matched.get("network", "mainnet") == network, "matched identity/profile mismatch"
    assert matched["config_sha256"] == hashlib.sha256((work / "daemon.json").read_bytes()).hexdigest()
    if args.role == "rust":
        assert matched["ocaml_graphql_genesis_hash_confirmed"], "check OCaml's live genesis hash first"
    config = json.loads((work / "daemon.json").read_text())
    assert not any(k in config["daemon"] for k in ("peer_list_url", "peer_list_file", "peers"))
    assert config["proof"]["level"] == "full"
    rust_http, rust_p2p = 3400 + 10 * args.rust_slot, 8600 + 10 * args.rust_slot
    ocaml_http, ocaml_p2p = 3400 + 10 * args.ocaml_slot, 8600 + 10 * args.ocaml_slot
    ports = (rust_http, rust_p2p) if args.role == "rust" else (ocaml_http, ocaml_p2p, ocaml_p2p + 1)
    for port in ports:
        with socket.socket() as sock:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(("0.0.0.0", port))
    # Keep unrelated node/key/config settings out of the child. Never dump env.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("MINA_", "OCAML_"))
           and k not in ("PEER_LIST_URL", "PEER_LIST_FILE", "PRODUCER_KEY", "RUN_SNARKER", "CONFIG", "GENESIS_CONFIG")}
    password = (work / "keys/password").read_text()
    env.update(MINA_PRIVKEY_PASS=password, MINA_LIBP2P_PASS=password,
               MINA_PROFILE=network, TMPDIR=str(work / "tmp"),
               MINA_CIRCUIT_BLOBS_BASE_DIR=str(args.blobs.resolve()),
               MINA_LIBP2P_DISCOVERY_MIN_INTERVAL="10m", RUST_BACKTRACE="1")
    peerids = {role: (work / "keys" / f"{role}-p2p.peerid").read_text().strip()
               for role in ("rust", "ocaml")}
    env.update(RAYON_NUM_THREADS="8" if args.role == "rust" else "2",
               OMP_NUM_THREADS="8" if args.role == "rust" else "2")
    if args.role == "rust":
        blob_version = "4.0.2devnet" if network == "devnet" else "4.0.2mainnet"
        assert (args.blobs / blob_version).is_dir(), f"missing {blob_version} circuits"
        command = [str(args.mina.resolve()), "--network", network, "node",
                   "--config", str(work / "daemon.json"),
                   "--work-dir", str(work / "rust-netns"), "--port", str(rust_http),
                   "--libp2p-port", str(rust_p2p), "--libp2p-external-ip", "127.0.0.46",
                   "--libp2p-keypair", str(work / "keys/rust-p2p"),
                   "--peer", f"/ip4/127.0.0.47/tcp/{ocaml_p2p}/p2p/{peerids['ocaml']}",
                   # max_stable_peers is floor(0.8 * max_peers). With 1, even
                   # the sole peer is evicted after 90 seconds.
                   "--no-peers-discovery", "--max-peers", "3" if args.relay_only else "2",
                   # P6 disables HTTP submissions by default. This isolated
                   # rehearsal explicitly submits a throwaway-account payment.
                   "--enable-tx-submission",
                   "--archive-local-storage", "--disable-filesystem-logging",
                   "--verbosity", "info" if args.rust_snark_worker_fee else "debug"]
        if args.rust_snark_worker_fee:
            worker = (work / "keys" / f"{args.snark_worker_key}.pub").read_text().strip()
            command.extend(("--run-snarker", worker, "--snarker-fee", args.rust_snark_worker_fee))
        elif not args.relay_only:
            command.extend(("--producer-key", str(work / "keys/producer")))
        env["MINA_ARCHIVE_LOCAL_STORAGE_PATH"] = str(work / "blocks")
    else:
        # Equivalent to scripts/ocaml-isolated-daemon.sh. Rust's outbound socket
        # uses 127.0.0.1 even when it advertises 127.0.0.46, so OCaml must trust
        # that source IP. The unique chain-ID PSK excludes other local networks.
        command = [str(root / "scripts/ocaml-mina"), "daemon",
                   "--config-directory", str(work / "ocaml-netns"),
                   "--config-file", str(work / "daemon.json"),
                   "--genesis-ledger-dir", str(work / "genesis"),
                   "--libp2p-keypair", str(work / "keys/ocaml-p2p"),
                   "--peer", f"/ip4/127.0.0.1/tcp/{rust_p2p}/p2p/{peerids['rust']}",
                   "--isolate-network", "true", "--proof-level", "full",
                   "--external-ip", "127.0.0.47", "--bind-ip", "127.0.0.47",
                   "--external-port", str(ocaml_p2p), "--client-port", str(ocaml_p2p + 1),
                   "--rest-port", str(ocaml_http), "--min-connections", "1", "--max-connections", "1",
                   "--enable-peer-exchange", "false", "--disable-node-status",
                   "--file-log-level", "Trace", "--log-level", "Info"]
        if args.ocaml_snark_worker:
            command += ["--run-snark-worker", (work / "keys/small.pub").read_text().strip(),
                        "--snark-worker-fee", "0.01", "--snark-worker-parallelism", "2",
                        "--work-selection", "seq"]
        if args.ocaml_producer:
            command.extend(("--block-producer-key", str(work / "keys/producer")))
    (work / f"{args.role}.pid").write_text(str(os.getpid()) + "\n")
    os.execvpe(command[0], command, env)


if __name__ == "__main__":
    main()
