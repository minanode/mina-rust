#!/usr/bin/env python3
"""Validate hosted circuit assets and print canonical build pins.

Prover files must match the OCaml manifest. The two verifier caches and the
Lagrange basis caches retain build-specific hashes. Canonical prover pins are
emitted even for observer builds that omit the prover files, so a later role
cannot load unpinned bytes.
"""
import argparse
import hashlib
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
CACHES = {"block_verifier_index.postcard", "transaction_verifier_index.postcard"}
# Lagrange basis caches (crates/ledger/src/proofs/lagrange_cache.rs).
LAGRANGE = re.compile(r"lagrange_basis_(?:pallas|vesta)_[0-9]{1,2}\.bin")


def is_cache(name):
    return name in CACHES or LAGRANGE.fullmatch(name) is not None


def manifest(network):
    text = (ROOT / f"tools/mesa-circuits/{network}-blobs.sha256").read_text()
    pins = {}
    for line in text.splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9_-]+\.(?:json|bin))", line)
        if not match or match[2] in pins:
            raise ValueError("invalid or duplicate canonical circuit manifest entry")
        pins[match[2]] = match[1]
    if len(pins) != 24:
        raise ValueError("expected 24 canonical prover files")
    return pins


def build_pins(network, directory, require_provers=False):
    pins = manifest(network)
    files = {p.name: p for p in directory.iterdir()}
    unknown = {name for name in files.keys() - pins.keys() if not is_cache(name)}
    if unknown:
        raise ValueError(f"unknown circuit file: {sorted(unknown)[0]}")
    present = files.keys() & pins.keys()
    if (require_provers or present) and present != pins.keys():
        raise ValueError(f"missing canonical prover file: {sorted(pins.keys() - present)[0]}")
    for name, path in sorted(files.items()):
        if not path.is_file():
            raise ValueError(f"circuit asset is not a file: {name}")
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if name in pins and digest != pins[name]:
            raise ValueError(f"canonical SHA-256 mismatch: {name}")
        # Prover hashes stay manifest-derived; only caches get staged hashes.
        if is_cache(name):
            pins[name] = digest
    return ";".join(f"4.0.2{network}/{name}={digest}" for name, digest in sorted(pins.items()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("network", choices=("devnet", "mainnet"))
    parser.add_argument("directory", type=Path, help="the selected network's asset directory")
    parser.add_argument("--require-provers", action="store_true")
    args = parser.parse_args()
    try:
        print(build_pins(args.network, args.directory, args.require_provers))
    except (ValueError, OSError) as error:
        parser.exit(1, f"web_pins: {error}\n")


if __name__ == "__main__":
    main()
