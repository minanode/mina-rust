#!/usr/bin/env python3
"""Negative controls for the differential runner, not consensus oracles."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUNNER = Path(__file__).with_name("run.py")


class RunnerControls(unittest.TestCase):
    def run_control(self, mutation, epoch=False):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            script = '''#!/usr/bin/env python3
import json,sys
for line in sys.stdin:
    case=json.loads(line)
    value={"value":case.get("slot",0)}
    if case["op"]=="build_info": value={"git":{"commit_hash":"control"}}
    if case["op"]=="epoch_select":
        data=case["state"]["next_epoch_data"]
        value=({"seed":data["seed"],"ledger":data["ledger"]["hash"],
                "total_currency":data["ledger"]["total_currency"]} if IS_RUST else data)
    if CONDITION:
        MUTATION
    print(json.dumps(value),flush=True)
'''
            for name, change in [("ocaml", "pass"), ("rust", mutation)]:
                p = root / name
                p.write_text(script.replace("MUTATION", change).replace("IS_RUST", str(name == "rust")).replace(
                    "CONDITION", 'case["op"]=="epoch_select"' if epoch else 'case["op"]=="arithmetic" and case["slot"]==7'))
                p.chmod(0o755)
            result = subprocess.run([sys.executable, "-B", str(RUNNER),
                "--rust", str(root/"rust"), "--ocaml", str(root/"ocaml"),
                "--output", str(root/"result"), "--network", "mainnet",
                "--scale", "0", "--only", "epoch_precompute" if epoch else "arithmetic"], capture_output=True, timeout=30)
            return result.returncode, json.loads((root/"result/report.json").read_text()), json.loads((root/"result/divergences.json").read_text())

    def test_agreement_records_counts_and_clean_workers(self):
        code, report, failures = self.run_control("pass")
        self.assertEqual(code, 0)
        self.assertTrue(report["passed"])
        self.assertEqual(report["networks"]["mainnet"]["counts"]["arithmetic"], 26)
        self.assertEqual(report["worker_exits"]["mainnet"], [0, 0])
        self.assertEqual(failures, [])

    def test_changed_result_fails_with_exact_reproduction(self):
        code, report, failures = self.run_control('value["value"]+=1')
        self.assertNotEqual(code, 0)
        self.assertFalse(report["passed"])
        self.assertEqual(report["mismatches"], 1)
        self.assertEqual(failures[0]["case"], {"op":"arithmetic", "slot":7})
        self.assertEqual(failures[0]["rust"]["value"], 8)
        self.assertEqual(failures[0]["ocaml"]["value"], 7)

    def test_epoch_payload_mutations_fail_with_unchanged_context_selection(self):
        code, report, failures = self.run_control("pass", epoch=True)
        self.assertEqual(code, 0)
        self.assertEqual(report["networks"]["mainnet"]["counts"]["epoch_precompute"], 25)
        for field, replacement in [("seed", "wrong-seed"), ("ledger", "wrong-ledger"),
                                   ("total_currency", "1")]:
            with self.subTest(field=field):
                code, report, failures = self.run_control(
                    f'value[{field!r}]={replacement!r}', epoch=True)
                self.assertNotEqual(code, 0)
                self.assertFalse(report["passed"])
                self.assertEqual(report["mismatches"], 25)
                self.assertNotEqual(failures[0]["rust"][field], failures[0]["ocaml"][field])

    def test_adapter_error_cannot_pass(self):
        code, report, _ = self.run_control('value={"error":"injected adapter failure"}')
        self.assertNotEqual(code, 0)
        self.assertFalse(report["passed"])
        self.assertIn("injected adapter failure", report["error"])

    def test_adapter_exit_cannot_pass(self):
        code, report, _ = self.run_control('sys.exit(9)')
        self.assertNotEqual(code, 0)
        self.assertFalse(report["passed"])
        self.assertIn("error", report)
        self.assertEqual(report["worker_exits"]["mainnet"][0], 9)


if __name__ == "__main__":
    unittest.main()
