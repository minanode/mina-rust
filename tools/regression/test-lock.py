#!/usr/bin/env python3
"""Bounded build-lock regressions using isolated lock files."""
import fcntl
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

LOCK = Path(__file__).resolve().with_name("lock.sh")


class BuildLockTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = {**os.environ, "MINA_LOCK_DIR": str(self.root),
                    "LOCK_WAIT": "0", "LOCK_POLL": "0.01"}
        self.env.pop("MINA_NO_LOCK", None)
        self.env.pop("MINA_BUILD_LOCK_FILE", None)
        self.env.pop("MINA_BUILD_LOCK_DEPTH", None)

    def hold(self, slot):
        file = (self.root / f"build-{slot}.lock").open("a")
        self.addCleanup(file.close)
        fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def run_lock(self, *command, **kwargs):
        return subprocess.run([str(LOCK), "build", "--", *command],
                              env=self.env, capture_output=True, text=True,
                              timeout=5, **kwargs)

    def test_nested_build_uses_the_only_free_slot(self):
        self.hold(2)
        self.hold(3)
        result = self.run_lock(str(LOCK), "build", "--", str(LOCK), "build", "--", "true")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr.count("took build-"), 1)
        self.assertEqual(self.run_lock("true").returncode, 0)

    def test_concurrent_siblings_are_serialized(self):
        self.hold(2)
        self.hold(3)
        self.env["LOCK_WAIT"] = "3"
        # An independent nonblocking lock detects overlapping command lifetimes.
        job = self.root / "job.sh"
        job.write_text('exec 7>"$1/active"\n'
                       'flock -n 7 || exit 42\n'
                       'echo start >>"$1/events"\n'
                       'sleep 0.2\n'
                       'echo end >>"$1/events"\n')
        for depth in (0, 1):
            with self.subTest(depth=depth):
                events = self.root / "events"
                events.unlink(missing_ok=True)
                command = ["bash", "-c",
                    '\"$1\" build -- bash "$2/job.sh" "$2" & a=$!; '
                    '\"$1\" build -- bash "$2/job.sh" "$2" & b=$!; '
                    'wait "$a"; x=$?; wait "$b"; y=$?; '
                    'test "$x" = 0 && test "$y" = 0',
                    "bash", str(LOCK), str(self.root)]
                if depth:
                    command = [str(LOCK), "build", "--", *command]
                result = self.run_lock(*command)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(events.read_text().splitlines(),
                                 ["start", "end", "start", "end"])
                self.assertEqual(self.run_lock("true").returncode, 0)

    def test_nested_exit_status_and_log(self):
        self.hold(2)
        self.hold(3)
        log = self.root / "output"
        result = self.run_lock(str(LOCK), "build", "--log", str(log), "--",
                               "bash", "-c", "echo nested; exit 17")
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertEqual(log.read_text(), "nested\n")

    def test_marker_without_descriptor_cannot_bypass_full_slots(self):
        for slot in (1, 2, 3):
            self.hold(slot)
        self.env["MINA_BUILD_LOCK_FILE"] = str(self.root / "build-1.lock")
        self.assertEqual(self.run_lock("true").returncode, 75)

    def test_closed_inherited_descriptor_does_not_bypass(self):
        self.hold(2)
        self.hold(3)
        result = self.run_lock("bash", "-c",
            '(exec 9>&-; "$1" build -- true); status=$?; exit "$status"',
            "bash", str(LOCK))
        self.assertEqual(result.returncode, 75, result.stderr)

    def test_other_directory_requires_its_own_slot(self):
        other = self.root / "other"
        other.mkdir()
        result = self.run_lock("env", f"MINA_LOCK_DIR={other}", str(LOCK), "build", "--", "true")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr.count("took build-"), 2)


if __name__ == "__main__":
    unittest.main()
