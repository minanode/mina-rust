#!/usr/bin/env python3
"""Portable checks against the committed OCaml prover bytes and manifests."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from web_pins import ROOT, build_pins, manifest


class WebPinsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temp.name)
        for network in ("devnet", "mainnet"):
            directory = cls.root / network
            directory.mkdir()
            for name in manifest(network):
                frame = ROOT / "crates/ledger/src/proofs/data/circuits" / f"4.0.2{network}" / (name + ".zst")
                with (directory / name).open("wb") as stream:
                    subprocess.run(["zstd", "-d", "-q", "-c", str(frame)], stdout=stream, check=True)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def test_canonical_match_both_networks(self):
        for network in ("devnet", "mainnet"):
            value = build_pins(network, self.root / network, require_provers=True)
            expected = {f"4.0.2{network}/{name}={digest}" for name, digest in manifest(network).items()}
            self.assertEqual(set(value.split(";")), expected)

    def test_mismatch_missing_and_unknown_fail_both_networks(self):
        for network in ("devnet", "mainnet"):
            for fault, message in (("changed", "SHA-256 mismatch"), ("missing", "missing canonical"), ("unknown", "unknown circuit")):
                with self.subTest(network=network, fault=fault), tempfile.TemporaryDirectory() as scratch:
                    directory = Path(scratch)
                    shutil.copytree(self.root / network, directory, dirs_exist_ok=True)
                    first = directory / next(iter(manifest(network)))
                    if fault == "changed":
                        with first.open("r+b") as stream:
                            byte = stream.read(1)
                            stream.seek(0)
                            stream.write(bytes([byte[0] ^ 1]))
                    elif fault == "missing":
                        first.unlink()
                    else:
                        (directory / "unknown_gates.json").write_bytes(b"unknown")
                    with self.assertRaisesRegex(ValueError, message):
                        build_pins(network, directory, require_provers=True)

    def test_observer_pins_canonical_provers_and_staged_cache(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            (directory / "block_verifier_index.postcard").write_bytes(b"abc")
            value = build_pins("devnet", directory)
            self.assertEqual(len(value.split(";")), 25)
            self.assertIn("block_verifier_index.postcard=ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", value)
            for name, digest in manifest("devnet").items():
                self.assertIn(f"4.0.2devnet/{name}={digest}", value)
            with self.assertRaisesRegex(ValueError, "missing canonical"):
                build_pins("devnet", directory, require_provers=True)


    def test_lagrange_caches_get_staged_pins(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            shutil.copytree(self.root / "mainnet", directory, dirs_exist_ok=True)
            (directory / "lagrange_basis_vesta_16.bin").write_bytes(b"abc")
            value = build_pins("mainnet", directory, require_provers=True)
            self.assertEqual(len(value.split(";")), 25)
            self.assertIn("4.0.2mainnet/lagrange_basis_vesta_16.bin=ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", value)
            for name in ("lagrange_basis_other_16.bin", "lagrange_basis_vesta_16.bin.partial", "lagrange_basis_vesta_123.bin"):
                with self.subTest(name=name), tempfile.TemporaryDirectory() as other:
                    (Path(other) / name).write_bytes(b"abc")
                    with self.assertRaisesRegex(ValueError, "unknown circuit"):
                        build_pins("mainnet", Path(other))


if __name__ == "__main__":
    unittest.main()
