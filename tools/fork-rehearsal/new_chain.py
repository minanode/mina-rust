#!/usr/bin/env python3
"""Prepare the next chain from the two exports, then compute each node's view
of its genesis independently.

1. `new/ocaml-base.json`: the old config without its ledgers, timestamp, fork
   and stop slots, for OCaml to lay its own `fork_validation/legacy/daemon.json`
   over (as OCaml's hard-fork test does). Rust reads only its own export.
2. `new/circuits/`: the new fork's circuits, dumped by OCaml 4.0.2 offline
   (`tools/mesa-circuits/generate.sh`, under the build lock), which Rust's
   prover and verifier need for a fork other than the release's.
3. `new/circuit-digests.json`: each blob checked against the digest in its
   name; `chain-id-digests.txt` next to the blobs: the chain id's digests
   traced from OCaml's `internal chain-id` on the new config (hashes only).
4. `new/rust-identity.json`: Rust's genesis state and chain id from
   `fork/rust/fork_config.json`.
5. OCaml's chain id and genesis state (with a full genesis proof) from its
   own export, and `new/identity-match.json`.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
LOCK = ROOT / "tools/regression/lock.sh"


def load(path):
    return json.loads(Path(path).read_text())


def normalize(value):
    # OCaml prints field elements in upper-case hex, Rust in lower case.
    if isinstance(value, str) and re.fullmatch(r"0x[0-9a-fA-F]+", value):
        return value.lower()
    if isinstance(value, list):
        return [normalize(v) for v in value]
    if isinstance(value, dict):
        return {k: normalize(v) for k, v in value.items()}
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--identity", type=Path, required=True,
                        help="local_mesa_identity binary")
    parser.add_argument("--digests", type=Path, required=True,
                        help="circuit_digests example binary")
    parser.add_argument("--skip-circuits", action="store_true",
                        help="new/circuits already generated")
    args = parser.parse_args()
    work = args.dir.resolve()
    network = load(work / "network.json")["network"]
    new = work / "new"
    new.mkdir(exist_ok=True)
    rust_manifest = load(work / "fork/rust/manifest.json")
    legacy = work / "fork/ocaml/fork_validation/legacy"
    ocaml_config = load(legacy / "daemon.json")
    fork = rust_manifest["daemon"]["proof"]["fork"]
    timestamp = rust_manifest["daemon"]["genesis"]["genesis_state_timestamp"]
    assert ocaml_config["proof"]["fork"] == fork, "the exports disagree on the fork point"
    assert ocaml_config["genesis"]["genesis_state_timestamp"] == timestamp

    old = load(work / "daemon.json")
    base = json.loads(json.dumps(old))
    for key in ("ledger", "epoch_data"):
        base.pop(key)
    base["genesis"].pop("genesis_state_timestamp")
    base["proof"].pop("fork")
    for key in ("slot_tx_end", "slot_chain_end", "hard_fork_genesis_slot_delta"):
        base["daemon"].pop(key)
    (new / "ocaml-base.json").write_text(json.dumps(base, indent=2) + "\n")
    # OCaml unpacks tarballs next to them: keep the export itself untouched.
    genesis = new / "ocaml-genesis"
    if not genesis.exists():
        shutil.copytree(legacy / "genesis", genesis)

    circuits_config = {"genesis": {"genesis_state_timestamp": timestamp},
                       "proof": {"fork": fork}, "ledger": {"accounts": []}}
    if args.skip_circuits:
        # Dumped early from the fork block once slot_tx_end had passed: it
        # must be the fork point and timestamp both exports name.
        assert load(new / "fork-circuits.json") == circuits_config, \
            "the circuits were dumped for another fork point"
    else:
        (new / "fork-circuits.json").write_text(json.dumps(circuits_config, indent=2) + "\n")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("MINA_", "OCAML_"))}
    env.update(MINA_PROFILE=network)
    if not args.skip_circuits:
        with (work / "logs/new-circuits.log").open("w") as log:
            subprocess.run([str(LOCK), "build", "--", str(ROOT / "tools/mesa-circuits/generate.sh"),
                            str(new / "circuits"), str(new / "fork-circuits.json")],
                           env=dict(env, MESA_CIRCUITS_LIVE_KEY="none"), stdout=log,
                           stderr=subprocess.STDOUT, check=True)
    blobs = new / "circuits/circuit-blobs"
    # Each blob must hash to the digest OCaml's key cache named it by.
    with (new / "circuit-digests.json").open("w") as out:
        subprocess.run([str(args.digests), str(blobs / f"4.0.2{network}")], stdout=out,
                       check=True)
    # The chain id's digests, from OCaml's `internal chain-id` on the new
    # config (hashes only), saved where Rust looks for them.
    merged = json.loads(json.dumps(base))
    for key, value in load(work / "fork/rust/daemon.json").items():
        merged[key] = {**merged.get(key, {}), **value}
    (new / "chain-id-config.json").write_text(json.dumps(merged, indent=2) + "\n")
    digests_file = blobs / f"4.0.2{network}" / "chain-id-digests.txt"
    with digests_file.open("w") as out:
        subprocess.run([str(ROOT / "scripts/ocaml-chain-id-trace.sh"), network,
                        str(new / "chain-id-config.json")],
                       env=dict(env, TMPDIR=str(work / "tmp")), stdout=out, check=True)

    subprocess.run(["python3", str(ROOT / "tools/mesa-circuits/bind-fork-bundle.py"),
                    str(new / "fork-circuits.json"), str(digests_file.parent)], check=True)

    with (work / "logs/new-rust-identity.log").open("w") as log:
        subprocess.run([str(args.identity), "--network", network,
                        "--config", str(work / "fork/rust/fork_config.json"),
                        "--out", str(new / "rust-identity.json")],
                       env=dict(env, MINA_CIRCUIT_BLOBS_BASE_DIR=str(blobs)),
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    rust = load(new / "rust-identity.json")

    enter = ["bash", str(ROOT / "tools/local-mesa-net/enter.sh"), str(work)]
    ocaml_env = dict(env, TMPDIR=str(work / "tmp"))
    configs = []
    for path in (new / "ocaml-base.json", legacy / "daemon.json"):
        configs += ["--config-file", str(path)]
    with (work / "logs/new-ocaml-chain-id.log").open("w") as log:
        subprocess.run(enter + [str(ROOT / "scripts/ocaml-mina"), "internal", "chain-id",
                                *configs, "--config-directory", str(new / "identity-conf"),
                                "--genesis-ledger-dir", str(genesis)],
                       env=ocaml_env, stdout=log, stderr=subprocess.STDOUT, check=True)
    ocaml_id = re.findall(r"[0-9a-f]{64}", (work / "logs/new-ocaml-chain-id.log").read_text())[-1]
    with (work / "logs/new-ocaml-genesis-proof.log").open("w") as log:
        subprocess.run(enter + [str(ROOT / "scripts/ocaml-mina"), "internal",
                                "test-genesis-block-generation", *configs,
                                "--config-directory", str(new / "proof-conf"),
                                "--genesis-ledger-dir", str(genesis)],
                       env=ocaml_env, stdout=log, stderr=subprocess.STDOUT, check=True)
    lines = (work / "logs/new-ocaml-genesis-proof.log").read_text().splitlines()
    ocaml_state = json.loads(lines[lines.index("Generated block") + 1])
    (new / "ocaml-genesis.json").write_text(json.dumps(ocaml_state, indent=2) + "\n")
    states_equal = normalize(ocaml_state["state"]) == normalize(rust["state"])
    result = {
        "rust_genesis_state_hash": rust["genesis_state_hash"],
        "rust_chain_id": rust["chain_id"],
        "ocaml_chain_id": ocaml_id,
        "chain_ids_equal": ocaml_id == rust["chain_id"],
        "genesis_states_equal": states_equal,
        "rust_blockchain_step_digest": rust["blockchain_step_digest"],
        "rust_config_sha256": hashlib.sha256(
            (work / "fork/rust/fork_config.json").read_bytes()).hexdigest(),
        "ocaml_config_sha256": hashlib.sha256((legacy / "daemon.json").read_bytes()).hexdigest(),
    }
    (new / "identity-match.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    assert states_equal and result["chain_ids_equal"], "the new genesis differs"


if __name__ == "__main__":
    main()
