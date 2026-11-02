"""Gate boundary checks; no node, keys or network are used."""

import argparse
import copy
import importlib.util
import json
import io
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location(
    "uptime_gate", Path(__file__).with_name("uptime-release-gate.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class GateTests(unittest.TestCase):
    def test_network_selection_reaches_the_reference_verifier(self):
        for network, signature_kind in (("mainnet", "mainnet"), ("devnet", "testnet")):
            with self.subTest(network=network):
                args = gate.argument_parser().parse_args([
                    "--binary", "mina", "--commit", "test", "--verifier", "verifier",
                    "--inspect", "inspect", "--output", "output", "--network", network])
                self.assertEqual(args.network, network)
                with patch.object(gate, "run") as run:
                    gate.verify_reports(Path("reports"), Path("inspect"), Path("verifier"),
                                        Path("output"), {"MINA_PROFILE": network}, None, network)
                command = run.call_args.args
                self.assertEqual(command[command.index("--network") + 1], network)
                config = Path(command[command.index("--config") + 1])
                self.assertTrue(config.is_file())
                self.assertEqual(json.loads(config.read_text())["daemon"]["network_id"], network)

    def test_mainnet_reference_processes_use_mainnet_and_reject_wrong_profile(self):
        # Exercise the real report harness with stand-in reference processes.
        # Each process refuses a testnet argument, so a devnet-only path fails.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = root / "reports"
            reports.mkdir()
            (reports / "report.json").write_text(json.dumps({
                "version": 1, "data": {"created_at": "2026-10-29T00:00:00Z", "block": "test"},
                "submitter": "test"}))
            inspect = root / "inspect"
            inspect.write_text('''#!/usr/bin/python3
import json, os, sys
assert sys.argv[2:] == ["--signature-kind", "mainnet"]
assert os.environ["MINA_PROFILE"] == "mainnet"
print(json.dumps({"block_roundtrip_exact": True, "http_signature_valid": True,
                  "snark_work": None, "state_hash": "test", "height": 1}))
''')
            inspect.chmod(0o700)
            verifier = root / "verifier"
            verifier.write_text('''#!/usr/bin/python3
import json, sys
assert sys.argv[1:4] == ["stdin", "--signature-kind", "mainnet"]
for row in json.load(sys.stdin):
    row.update(verified=True, validation_error=None, state_hash=row["block_hash"], height=1)
    for key in list(row):
        if row[key] == "NOT_VALIDATED": row[key] = None
    print(json.dumps(row))
''')
            verifier.chmod(0o700)
            with (root / "log").open("w") as log:
                for profile in ("devnet", "mainnet"):
                    output = root / profile
                    def verify():
                        gate.verify_reports(reports, inspect, verifier, output,
                                            dict(os.environ, MINA_PROFILE=profile, TMPDIR=str(root)),
                                            log, "mainnet")
                    if profile == "devnet":
                        with self.assertRaises(subprocess.CalledProcessError):
                            verify()
                    else:
                        verify()
                        result = json.loads((output / "results.json").read_text())
                        self.assertEqual(result["signature_kind"], "mainnet")
                        self.assertTrue(result["http_signatures_checked"])

    def test_explicit_ports_are_not_limited_to_the_original_lane(self):
        args = gate.argument_parser().parse_args([
            "--binary", "mina", "--commit", "test", "--verifier", "verifier",
            "--inspect", "inspect", "--output", "output",
            "--http-port", "4340", "--libp2p-port", "9460"])
        self.assertEqual((args.http_port, args.libp2p_port), (4340, 9460))
        for value in (1, 65535):
            self.assertEqual(gate.port_number(str(value)), value)
        for value in (-1, 0, 65536):
            with self.assertRaises(argparse.ArgumentTypeError):
                gate.port_number(str(value))

    def test_two_complete_intervals_and_three_reports_are_required(self):
        self.assertFalse(gate.report_window([], None, 10000))
        self.assertFalse(gate.report_window([1, 2, 3], 100, 999))
        self.assertFalse(gate.report_window([1, 2], 100, 1000))
        self.assertTrue(gate.report_window([1, 2, 3], 100, 1000))

    def test_smoke_run_does_not_claim_a_complete_release_window(self):
        self.assertFalse(gate.report_window([], None, 0, smoke=True))
        self.assertTrue(gate.report_window([1], 100, 100, smoke=True))
        self.assertFalse(gate.report_window([1], 100, 100))

    @staticmethod
    def sok_inspection(zkapp=True, zero=True):
        # Synthetic policy controls based on independently decoded OCaml
        # fields. These are not captured signed zkApp reports or proof tests.
        capture = gate.ROOT / "tools/uptime-fixtures/full-proof/devnet-581348-coinbase/decoded.json"
        inspection = json.loads(capture.read_text())
        inspection["http_signature_valid"] = True
        work = inspection["snark_work"]
        work.update(sok_matches=False, sok_is_default=zero,
                    proof_sok_digest_hex=("00" if zero else "11") * 32)
        if zkapp:
            local = work["statement"]["source"]["local_state"]
            local.update(account_update_index="2", transaction_commitment="0x01",
                         full_transaction_commitment="0x02")
        return inspection

    def verify_policy_control(self, inspection, *, network="devnet",
                              work_error="proof's sok message digest does not match the sok message",
                              block_error=None, valid_work_proof=True):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = root / "reports"
            reports.mkdir()
            (reports / "report.json").write_text(json.dumps({
                "version": 1, "data": {"created_at": "2026-10-29T00:00:00Z",
                                      "block": "stub block", "snark_work": "c3R1YiB3b3Jr"},
                "submitter": "stub submitter"}))
            inspect = root / "inspect"
            inspect.write_text("#!/usr/bin/python3\nimport json\nprint(json.dumps(" + repr(inspection) + "))\n")
            inspect.chmod(0o700)
            verifier = root / "verifier"
            verifier.write_text("#!/usr/bin/python3\nimport json,sys\n" +
                "work_error=" + repr(work_error) + "\nblock_error=" + repr(block_error) + "\n" +
                "height=" + repr(inspection["height"]) + "\n" + '''
for row in json.load(sys.stdin):
    error = work_error if row["snark_work"] is not None else block_error
    row.update(verified=True, validation_error=error,
               state_hash=None if error else row["block_hash"], height=None if error else height,
               parent=None, slot=None)
    print(json.dumps(row))
''')
            verifier.chmod(0o700)
            work_verifier = root / "mina"
            work_verifier.write_text("#!/usr/bin/python3\nimport sys\n" +
                                     ("print('uptime work proof verified')\n" if valid_work_proof else "sys.exit(1)\n"))
            work_verifier.chmod(0o700)
            output = root / "output"
            with (root / "log").open("w") as log:
                gate.verify_reports(reports, inspect, verifier, output,
                                    dict(os.environ, MINA_PROFILE=network, TMPDIR=str(root),
                                         PYTHONOPTIMIZE="1"), log, network, work_verifier)
            return json.loads((output / "results.json").read_text())

    def test_zero_sok_terminal_zkapp_is_accepted_and_recorded_on_both_networks(self):
        for network in ("devnet", "mainnet"):
            with self.subTest(network=network):
                result = self.verify_policy_control(self.sok_inspection(), network=network)
                self.assertEqual(len(result["results"]), 2)
                self.assertEqual(len(result["exceptions"]), 1)
                exception = result["exceptions"][0]
                self.assertEqual(exception["classification"], "decoded_work_statement")
                self.assertTrue(exception["block_verified"])
                self.assertTrue(exception["http_signature_verified"])
                self.assertTrue(exception["work_proof_verified"])
                self.assertFalse(exception["work_and_sok_fully_verified"])
                self.assertEqual(exception["case"], "report.json")

    def test_zero_sok_ordinary_job_is_rejected_despite_a_claimed_zkapp_label(self):
        inspection = self.sok_inspection(zkapp=False)
        inspection["zkapp"] = True
        with self.assertRaises(subprocess.CalledProcessError):
            self.verify_policy_control(inspection)

    def test_wrong_submitter_nonzero_sok_is_rejected(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.verify_policy_control(self.sok_inspection(zero=False))

    def test_nonterminal_or_ambiguous_zero_sok_work_is_rejected(self):
        for nonterminal in (True, False):
            inspection = self.sok_inspection()
            statement = inspection["snark_work"]["statement"]
            if nonterminal:
                statement["target"]["local_state"] = copy.deepcopy(statement["source"]["local_state"])
            else:
                statement["source"]["local_state"] = copy.deepcopy(statement["target"]["local_state"])
            with self.assertRaises(subprocess.CalledProcessError):
                self.verify_policy_control(inspection)

    def test_zkapp_exception_cannot_mask_a_bad_block_or_other_verifier_error(self):
        for errors in ({"block_error": "bad blockchain proof"}, {"work_error": "other error"},
                       {"valid_work_proof": False}):
            with self.assertRaises(subprocess.CalledProcessError):
                self.verify_policy_control(self.sok_inspection(), **errors)

    def test_zkapp_exception_still_requires_signature_roundtrips_and_target(self):
        for field in ("http_signature_valid", "block_roundtrip_exact", "roundtrip_exact", "target_matches_block"):
            inspection = self.sok_inspection()
            owner = inspection if field in inspection else inspection["snark_work"]
            owner[field] = False
            with self.assertRaises(subprocess.CalledProcessError):
                self.verify_policy_control(inspection)

    def test_bound_work_still_requires_full_verification(self):
        inspection = self.sok_inspection(zkapp=False, zero=False)
        inspection["snark_work"]["sok_matches"] = True
        accepted = self.verify_policy_control(inspection, work_error=None)
        self.assertEqual(accepted["exceptions"], [])
        self.assertEqual(len(accepted["results"]), 1)
        with self.assertRaises(subprocess.CalledProcessError):
            self.verify_policy_control(inspection, work_error="bad work proof")

    def test_genesis_and_pre_fork_reports_fail_the_gate(self):
        for height in (1, 545433, 545434):
            with self.assertRaises(RuntimeError):
                gate.check_report_heights([{"height": height}], 545434)
        gate.check_report_heights([{"height": 581790}], 545434)
        with self.assertRaises(RuntimeError):
            gate.check_report_heights([], 545434)

    def test_genesis_threshold_comes_from_the_selected_network(self):
        # Fork heights are pinned in the OCaml runtime configs. In particular,
        # the devnet threshold must not admit mainnet's higher genesis block.
        for network, expected in (("devnet", 545434), ("mainnet", 548147)):
            with self.subTest(network=network):
                threshold = gate.genesis_height(network)
                self.assertEqual(threshold, expected)
                for height in (1, 545434, expected - 1, expected):
                    with self.assertRaises(RuntimeError):
                        gate.check_report_heights([{"height": height}], threshold)
                gate.check_report_heights([{"height": expected + 1}], threshold)

    def test_restart_requires_completed_restore_not_just_an_open_store(self):
        self.assertFalse(gate.restored_frontier("Idle"))
        self.assertFalse(gate.restored_frontier([{"Ready": {"restore": "Idle"}}]))
        self.assertFalse(gate.restored_frontier([{"Ready": {"restore": {"Done": {"outcome": "Off"}}}}]))
        self.assertFalse(gate.restored_frontier([{"Ready": {"restore": {"LoadPending": {}}}}]))
        self.assertFalse(gate.restored_frontier([{"Ready": {"restore": {"Done": {
            "outcome": {"Absent": {}}}}}}]))
        self.assertTrue(gate.restored_frontier([{"Ready": {"restore": {"Done": {
            "outcome": {"Restored": {}}}}}}]))

    def test_optional_rpc_timeouts_do_not_fabricate_frontier_evidence(self):
        opener = Mock()
        opener.open.side_effect = TimeoutError("slow block application")
        sample = gate.sample_frontier(opener, 4610, True)
        self.assertIsNone(sample["frontier"])
        self.assertIsNone(sample["persist"])
        self.assertEqual(set(sample["sample_errors"]), {"frontier", "persist"})
        self.assertFalse(gate.restored_frontier(sample["persist"]))

    def test_state_endpoint_is_only_read_when_restore_evidence_is_needed(self):
        opener = Mock()
        opener.open.return_value = io.StringIO('{"transition_frontier": {"best_tip": null}}')
        sample = gate.sample_frontier(opener, 4610, False)
        opener.open.assert_called_once_with("http://127.0.0.1:4610/status", timeout=5)
        self.assertEqual(sample["frontier"], {"best_tip": None})
        self.assertIsNone(sample["persist"])
        self.assertEqual(sample["sample_errors"], {})

    def test_shutdown_report_stays_in_the_pre_restart_file_set(self):
        with tempfile.TemporaryDirectory() as directory:
            reports = Path(directory)
            (reports / "early.json").write_text("{}")
            def finish_report(*command):
                self.assertEqual(command, ("systemctl", "--user", "stop", "owned-test-unit"))
                (reports / "during-stop.json").write_text("{}")
            with patch.object(gate, "run", side_effect=finish_report):
                finalized = gate.stop_node("owned-test-unit", reports)
            self.assertEqual({path.name for path in finalized},
                             {"early.json", "during-stop.json"})

    def test_http_attempts_and_other_reporters_fail_closed(self):
        status = {"uptime": {"attempts": 0}, "node_status": None, "node_error": None}
        gate.check_status(status)
        status["uptime"]["attempts"] = 1
        with self.assertRaises(RuntimeError):
            gate.check_status(status)

        status["uptime"]["attempts"] = 0
        status["node_error"] = {}
        with self.assertRaises(RuntimeError):
            gate.check_status(status)

    def test_optimization_cannot_disable_signature_checks(self):
        # The decoder deliberately rejects a signature while the fake proof
        # verifier would accept everything. The real harness must reject it,
        # even when the release manager's environment requests Python -O.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = root / "reports"
            reports.mkdir()
            (reports / "report.json").write_text(json.dumps({
                "version": 1, "data": {"created_at": "2026-10-29T00:00:00Z", "block": "test"},
                "submitter": "test"}))
            inspect = root / "inspect"
            inspect.write_text("#!/usr/bin/python3\nimport json\nprint(json.dumps(" + repr({
                "block_roundtrip_exact": True, "http_signature_valid": False,
                "snark_work": None, "state_hash": "test"}) + "))\n")
            inspect.chmod(0o700)
            verifier = root / "verifier"
            verifier.write_text('''#!/usr/bin/python3
import json, sys
for row in json.load(sys.stdin):
    row.update(verified=True, validation_error=None, state_hash=row["block_hash"], height=1)
    for key in list(row):
        if row[key] == "NOT_VALIDATED": row[key] = None
    print(json.dumps(row))
''')
            verifier.chmod(0o700)
            with (root / "log").open("w") as log:
                with self.assertRaises(subprocess.CalledProcessError):
                    gate.verify_reports(reports, inspect, verifier, root / "output",
                                        dict(os.environ, MINA_PROFILE="devnet", TMPDIR=str(root),
                                             PYTHONOPTIMIZE="1"), log)
            self.assertIn("http_signature_valid", (root / "log").read_text())


if __name__ == "__main__":
    unittest.main()
