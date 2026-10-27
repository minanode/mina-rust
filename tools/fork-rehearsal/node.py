#!/usr/bin/env python3
"""Run one rehearsal node inside the run's loopback-only namespace.

`--phase old` runs the old chain from `daemon.json`. `--phase new` runs the
next chain from each node's own export: Rust from `fork/rust/fork_config.json`
with the new fork's circuits, OCaml from its `generate-hardfork-config`
validation output (the stable ledgers 4.0.2 can load) over the old config's
constants. Each phase and node has its own state directory; the old ones are
kept. Run through `launch.py`, never in the host namespace.
"""

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess

RUST_HTTP, RUST_P2P = 3660, 8860
OCAML_HTTP, OCAML_P2P, OCAML_CLIENT = 3670, 8870, 8871


def check_isolation(work, name):
    interfaces = json.loads(subprocess.check_output(["ip", "-json", "address"], text=True))
    routes = json.loads(subprocess.check_output(
        ["ip", "-json", "route", "show", "table", "all"], text=True))
    routes6 = json.loads(subprocess.check_output(
        ["ip", "-6", "-json", "route", "show", "table", "all"], text=True))
    assert [row["ifname"] for row in interfaces] == ["lo"], "not a loopback-only namespace"
    assert all(row.get("dev") == "lo" and row.get("dst") != "default"
               for row in routes + routes6), "an external route exists"
    (work / f"{name}-isolation-preflight.json").write_text(json.dumps({
        "interfaces": interfaces, "routes": routes, "routes6": routes6,
    }, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("role", choices=("rust", "ocaml"))
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--phase", choices=("old", "new"), required=True)
    parser.add_argument("--mina", type=Path, help="Rust mina binary")
    parser.add_argument("--blobs", type=Path, help="Rust circuit blobs base directory")
    parser.add_argument("--producer", help="key name under keys/ to produce with")
    args = parser.parse_args()
    os.umask(0o077)
    root = Path(__file__).resolve().parents[2]
    work = args.dir.resolve()
    name = f"{args.role}-{args.phase}"
    namespace = json.loads((work / "namespace.json").read_text())
    assert os.readlink("/proc/self/ns/net") == namespace["netns"], "run through launch.py"
    check_isolation(work, name)
    for port in ((RUST_HTTP, RUST_P2P) if args.role == "rust"
                 else (OCAML_HTTP, OCAML_P2P, OCAML_CLIENT)):
        with socket.socket() as sock:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(("0.0.0.0", port))
    network = json.loads((work / "network.json").read_text())["network"]
    password = (work / "keys/password").read_text()
    # Keep unrelated node/key/config settings out of the child. Never dump env.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("MINA_", "OCAML_"))
           and k not in ("PEER_LIST_URL", "PEER_LIST_FILE", "PRODUCER_KEY", "RUN_SNARKER",
                         "CONFIG", "GENESIS_CONFIG")}
    env.update(MINA_PRIVKEY_PASS=password, MINA_LIBP2P_PASS=password,
               MINA_PROFILE=network, TMPDIR=str(work / "tmp"),
               MINA_LIBP2P_DISCOVERY_MIN_INTERVAL="10m", RUST_BACKTRACE="1")
    peerids = {role: (work / "keys" / f"{role}-p2p.peerid").read_text().strip()
               for role in ("rust", "ocaml")}
    if args.role == "rust":
        assert args.mina and args.blobs, "rust needs --mina and --blobs"
        assert (args.blobs / f"4.0.2{network}").is_dir(), "missing circuits"
        env["MINA_CIRCUIT_BLOBS_BASE_DIR"] = str(args.blobs.resolve())
        config = (work / "daemon.json" if args.phase == "old"
                  else work / "fork/rust/fork_config.json")
        command = [str(args.mina.resolve()), "--network", network, "node",
                   "--config", str(config),
                   "--work-dir", str(work / name), "--port", str(RUST_HTTP),
                   "--libp2p-port", str(RUST_P2P), "--libp2p-external-ip", "127.0.0.46",
                   "--libp2p-keypair", str(work / "keys/rust-p2p"),
                   "--peer", f"/ip4/127.0.0.47/tcp/{OCAML_P2P}/p2p/{peerids['ocaml']}",
                   # A maximum of 1 evicts the only peer (P4.6c-F2).
                   "--no-peers-discovery", "--max-peers", "2",
                   "--enable-tx-submission", "--enable-ledger-export",
                   "--archive-local-storage", "--disable-filesystem-logging",
                   "--verbosity", "debug"]
        if args.producer:
            command.extend(("--producer-key", str(work / "keys" / args.producer)))
        env["MINA_ARCHIVE_LOCAL_STORAGE_PATH"] = str(work / f"{name}-blocks")
    else:
        if args.phase == "old":
            configs = [work / "daemon.json"]
            genesis_dir = work / "genesis"
        else:
            legacy = work / "fork/ocaml/fork_validation/legacy"
            configs = [work / "new/ocaml-base.json", legacy / "daemon.json"]
            genesis_dir = work / "new/ocaml-genesis"
        command = [str(root / "scripts/ocaml-mina"), "daemon",
                   "--config-directory", str(work / name)]
        for config in configs:
            command.extend(("--config-file", str(config)))
        command += ["--genesis-ledger-dir", str(genesis_dir),
                    "--libp2p-keypair", str(work / "keys/ocaml-p2p"),
                    "--peer", f"/ip4/127.0.0.1/tcp/{RUST_P2P}/p2p/{peerids['rust']}",
                    "--isolate-network", "true", "--proof-level", "full",
                    "--external-ip", "127.0.0.47", "--bind-ip", "127.0.0.47",
                    "--external-port", str(OCAML_P2P), "--client-port", str(OCAML_CLIENT),
                    "--rest-port", str(OCAML_HTTP), "--min-connections", "1",
                    "--max-connections", "1", "--enable-peer-exchange", "false",
                    "--disable-node-status", "--file-log-level", "Trace",
                    "--log-level", "Info"]
        if args.producer:
            command.extend(("--block-producer-key", str(work / "keys" / args.producer)))
    (work / f"{name}.pid").write_text(str(os.getpid()) + "\n")
    (work / f"{name}-command.json").write_text(json.dumps(command, indent=2) + "\n")
    os.execvpe(command[0], command, env)


if __name__ == "__main__":
    main()
