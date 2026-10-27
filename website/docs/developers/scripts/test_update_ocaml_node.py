#!/usr/bin/env python3
"""Exercise private and public updater inputs without network or builds."""
import subprocess
import tempfile
from pathlib import Path
import unittest


SCRIPT = Path(__file__).with_name("update-ocaml-node.sh")
CONFIG_FILES = (
    "tools/testing/src/node/ocaml/config.rs",
    "tools/testing/src/node/ocaml/mod.rs",
    "tools/testing/src/scenarios/multi_node/basic_connectivity_peer_discovery.rs",
    "tools/testing/src/scenarios/solo_node/basic_connectivity_accept_incoming.rs",
)


class UpdateOcamlNode(unittest.TestCase):
    def run_case(self, workflows):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(["git", "init", "-q", tmp], check=True)
            files = list(CONFIG_FILES) + ["docker-compose.archive.devnet.compare.yml"]
            files += [".github/workflows/" + name for name in workflows]
            for name in files:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("config_12345678 version=3.0.0-1234567\n")
            subprocess.run(
                ["bash", str(SCRIPT), "12345678", "87654321", "3.0.0", "3.1.0"],
                cwd=root, check=True, capture_output=True,
            )
            for name in CONFIG_FILES:
                self.assertIn("config_87654321", (root / name).read_text())
            for name in [CONFIG_FILES[0], files[4]] + files[5:]:
                self.assertIn("3.1.0-8765432", (root / name).read_text())
                self.assertNotIn("3.0.0-1234567", (root / name).read_text())
            for name in {"tests.yaml", "test-graphql-compatibility.yml"} - set(workflows):
                self.assertFalse((root / ".github/workflows" / name).exists())

    def test_both_private_workflows_updated(self):
        self.run_case(["tests.yaml", "test-graphql-compatibility.yml"])

    def test_single_optional_workflow_updated(self):
        self.run_case(["tests.yaml"])

    def test_export_without_workflows(self):
        self.run_case([])


if __name__ == "__main__":
    unittest.main()
