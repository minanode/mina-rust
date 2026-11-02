#!/usr/bin/env python3
"""Expose a JSON test hook without changing pinned OCaml protocol functions."""
import argparse
import hashlib
from pathlib import Path

SOURCE_COMMIT = "c9e2eac5344da621828c1e96c074b8bdce665b51"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    consensus = args.source / "src/lib/consensus"
    source = consensus / "proof_of_stake.ml"
    interface = consensus / "proof_of_stake_intf.ml"
    expected = {
        source: "7a6a018ad37aebe149edd8bc287113b1baa9a497b7c5032850d5c63d2af7fca3",
        interface: "68de23a994404cb5e1d8b17e6e0de6c028d25b4a97cd707f7cd70a5d7badfcc6",
    }
    for path, digest in expected.items():
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise SystemExit(f"source does not match OCaml {SOURCE_COMMIT}: {path.name}")
    original = source.read_text()
    if "let consensus_diff json" in original:
        raise SystemExit("source already instrumented")
    print("Original proof_of_stake.ml SHA-256:", hashlib.sha256(source.read_bytes()).hexdigest())
    anchor = "  module Body_reference = Body_reference\nend\n"
    assert original.count(anchor) == 1
    source.write_text(original.replace(anchor, (here / "ocaml/hook.ml").read_text() + "\n" + anchor))
    sig = interface.read_text()
    assert sig.startswith("module type Full = sig\n")
    interface.write_text(sig.replace("module type Full = sig\n", "module type Full = sig\n  val consensus_diff : Yojson.Safe.t -> Yojson.Safe.t\n", 1))
    app = args.source / "src/app/consensus_diff"
    app.mkdir(parents=True, exist_ok=True)
    for name in ("dune", "consensus_diff.ml"):
        (app / name).write_bytes((here / "ocaml" / name).read_bytes())


if __name__ == "__main__":
    main()
