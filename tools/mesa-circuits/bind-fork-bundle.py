#!/usr/bin/env python3
"""Bind trusted OCaml fork circuits and their saved chain-ID trace to a config.

Usage: bind-fork-bundle.py DUMP_CONFIG BLOB_DIRECTORY
DUMP_CONFIG must be the exact config used to dump these circuits with OCaml.
This records provenance; it does not authenticate an untrusted circuit dump.
Startup independently checks the fork, computed genesis/chain ID, and all hashes.
"""
import hashlib
import json
from pathlib import Path
import sys


def bind(config_path, directory):
    config = json.loads(config_path.read_text())
    trace = directory / "chain-id-digests.txt"
    lines = trace.read_text().splitlines()

    def value(prefix):
        values = [line[len(prefix):] for line in lines if line.startswith(prefix) and line != "chain_id printed by mina matches"]
        if len(values) != 1 or len(values[0].split()) != 1:
            raise ValueError(f"missing or duplicate {prefix!r} in {trace}")
        return values[0]

    files = sorted(p for suffix in ("*_gates.json", "*_internal_vars.bin", "*_rows_rev.bin")
                   for p in directory.glob(suffix))
    if len(files) != 24:
        raise ValueError(f"expected 24 circuit files, found {len(files)}")
    bundle = {
        "version": 1,
        "fork": config["proof"]["fork"],
        "genesis_state_hash": value("genesis_state_hash (zero currencies) "),
        "chain_id": value("chain_id "),
        "files": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [*files, trace]},
    }
    target = directory / "fork-bundle.json"
    temporary = target.with_suffix(".partial")
    temporary.write_text(json.dumps(bundle, indent=2, sort_keys=True) + "\n")
    temporary.replace(target)
    print(target)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    bind(Path(sys.argv[1]), Path(sys.argv[2]))
