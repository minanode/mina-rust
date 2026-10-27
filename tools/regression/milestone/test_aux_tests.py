#!/usr/bin/env python3
"""Check auxiliary syntax failure propagation without running nodes or builds."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class AuxTests(unittest.TestCase):
    def test_syntax_failures(self):
        runner = Path(__file__).with_name("aux-tests.sh")
        for language in ("clean", "bash", "python", "node", "private-pass",
                         "showcase-fail", "devnet-fail", "missing-checker"):
            with self.subTest(language=language), tempfile.TemporaryDirectory() as tmp:
                repo = Path(tmp)
                scripts = repo / "tools/regression/milestone"
                scripts.mkdir(parents=True)
                shutil.copyfile(runner, scripts / runner.name)
                (repo / "frontend").mkdir()
                devnet = repo / "tools/regression/devnet"
                devnet.mkdir()
                (devnet / "test-watch.sh").write_text("exit 0\n")
                (devnet / "fixture.sh").write_text(
                    "if\n" if language == "bash" else "true\n"
                )
                (devnet / "fixture.py").write_text(
                    "if\n" if language == "python" else "pass\n"
                )
                webnode = repo / "tools/regression/webnode"
                webnode.mkdir()
                (webnode / "fixture.mjs").write_text(
                    "const =;\n" if language == "node" else "const x = 1;\n"
                )
                private = language in ("private-pass", "showcase-fail", "devnet-fail", "missing-checker")
                if private:
                    for path, directory, failure in (
                        ("tools/regression/check_showcase_claims.py", "docs/showcase", "showcase-fail"),
                        ("docs/research/P4.6c-devnet/verify.py", "docs/research/P4.6c-devnet", "devnet-fail"),
                    ):
                        (repo / directory).mkdir(parents=True, exist_ok=True)
                        (repo / path).write_text(
                            "raise SystemExit(7)\n" if language == failure else "print('checked private evidence')\n"
                        )
                    if language == "missing-checker":
                        (repo / "tools/regression/check_showcase_claims.py").unlink()
                shim = repo / "bin"
                shim.mkdir()
                # Skip unrelated test suites; real interpreters perform syntax checks.
                for command, check_flag in (("python3", "-c"), ("node", "--check")):
                    executable = shutil.which(command)
                    script = shim / command
                    script.write_text(
                        f'#!/bin/sh\nif [ "$1" = "{check_flag}" ]; then\n'
                        f'  exec "{executable}" "$@"\nfi\n'
                        'case "$2" in */check_showcase_claims.py|*/P4.6c-devnet/verify.py)\n'
                        f'  exec "{executable}" "$@";;\nesac\nexit 0\n'
                    )
                    script.chmod(0o755)
                gate = repo / "gate"
                result = subprocess.run(
                    ["bash", str(scripts / runner.name)],
                    env={
                        **os.environ,
                        "PATH": f"{shim}:{os.environ['PATH']}",
                        "GATE_DIR": str(gate),
                    },
                    capture_output=True,
                    text=True,
                )
                success = language in ("clean", "private-pass")
                expected = 0 if success else 7 if language in ("showcase-fail", "devnet-fail") else 1
                self.assertEqual(result.returncode, expected, result.stderr)
                if not private:
                    log = (gate / "aux/syntax.log").read_text()
                    self.assertEqual("FAIL" in log, language != "clean")
                    for filename in ("showcase-claims.log", "devnet-evidence.log"):
                        self.assertIn("SKIP:", (gate / "aux" / filename).read_text())
                if language == "private-pass":
                    for filename in ("showcase-claims.log", "devnet-evidence.log"):
                        self.assertIn("checked private evidence", (gate / "aux" / filename).read_text())
                self.assertEqual((gate / "aux/done").exists(), success)
                print(
                    f"{language}: exit={result.returncode}, "
                    f"done={(gate / 'aux/done').exists()}"
                )


if __name__ == "__main__":
    unittest.main()
