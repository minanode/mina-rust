#!/usr/bin/env python3
"""Require independent OCaml and Rust genesis-state and chain-ID agreement."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import urllib.request

from network_profile import load_profile


def normalize(value):
    # OCaml emits uppercase field-element hex; Rust emits lowercase hex.
    if isinstance(value, str) and re.fullmatch(r"0x[0-9a-fA-F]+", value):
        return value.lower()
    if isinstance(value, list):
        return [normalize(item) for item in value]
    if isinstance(value, dict):
        return {key: normalize(item) for key, item in value.items()}
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--ocaml-port", type=int, help="confirm the running daemon's own state hash")
    args = parser.parse_args()
    work = args.dir
    network = load_profile(work)
    rust = json.loads((work / "rust-identity.json").read_text())
    assert rust.get("network", "mainnet") == network, "identity/profile mismatch"
    lines = (work / "logs/ocaml-genesis-proof.log").read_text().splitlines()
    ocaml = json.loads(lines[lines.index("Generated block") + 1])
    assert normalize(ocaml["state"]) == normalize(rust["state"]), "genesis states differ"
    chain_log = (work / "logs/ocaml-chain-id.log").read_text()
    ocaml_id = re.search(r"[0-9a-f]{64}$", chain_log.strip()).group()
    assert ocaml_id == rust["chain_id"], "chain IDs differ"
    (work / "ocaml-genesis.json").write_text(json.dumps(ocaml, indent=2) + "\n")
    result = {key: rust[key] for key in ("genesis_state_hash", "genesis_ledger_hash",
                                        "chain_id_genesis_hash", "chain_id")}
    result.update(network=network,
                  config_sha256=hashlib.sha256((work / "daemon.json").read_bytes()).hexdigest(),
                  genesis_state_comparison="all fields equal, ignoring hex letter case only",
                  ocaml_genesis_proof="generated with full proofs",
                  ocaml_graphql_genesis_hash_confirmed=False)
    if args.ocaml_port is not None:
        request = urllib.request.Request(
            f"http://127.0.0.1:{args.ocaml_port}/graphql",
            data=json.dumps({"query": "{ genesisBlock { stateHash } daemonStatus { chainId } }"}).encode(),
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=10) as response:
            observed = json.load(response)
        (work / "ocaml-initial-graphql.json").write_text(json.dumps(observed, indent=2) + "\n")
        assert not observed.get("errors"), observed.get("errors")
        assert observed["data"]["genesisBlock"]["stateHash"] == rust["genesis_state_hash"]
        assert observed["data"]["daemonStatus"]["chainId"] == rust["chain_id"]
        result["ocaml_graphql_genesis_hash_confirmed"] = True
    (work / "identity-match.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
