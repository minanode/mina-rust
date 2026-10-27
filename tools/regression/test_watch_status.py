import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import stat
import subprocess

spec = importlib.util.spec_from_file_location("watch_status", Path(__file__).with_name("watch-status.py"))
watch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(watch)


def info(size=10, mtime=1000, inode=1):
    return SimpleNamespace(st_dev=1, st_ino=inode, st_size=size,
                           st_mtime=mtime, st_mode=stat.S_IFREG)


class LogTests(unittest.TestCase):
    def check(self, growth, sample, now, wall):
        with patch.object(watch.os, "stat", return_value=sample):
            return growth.check("node.log", 60, now, wall)

    def test_stale_first_sample_and_boundary(self):
        g = watch.LogGrowth()
        self.assertIsNone(self.check(g, info(), 10, 1060))
        self.assertIn("not grown", self.check(g, info(), 11, 1061))

    def test_growth_recovers_and_touch_does_not(self):
        g = watch.LogGrowth()
        self.assertIsNone(self.check(g, info(), 0, 1000))
        self.assertIn("not grown", self.check(g, info(mtime=1061), 61, 1061))
        self.assertIsNone(self.check(g, info(size=11, mtime=1062), 62, 1062))

    def test_empty_truncated_and_rotated_files(self):
        g = watch.LogGrowth()
        self.assertIsNone(self.check(g, info(), 0, 1000))
        self.assertIn("empty", self.check(g, info(size=0), 61, 1061))
        self.assertIn("not grown", self.check(g, info(inode=2), 62, 1062))
        self.assertIsNone(self.check(g, info(size=11, mtime=1063, inode=2), 63, 1063))

    def test_missing_and_future_file(self):
        g = watch.LogGrowth()
        with patch.object(watch.os, "stat", side_effect=FileNotFoundError(2, "missing")):
            self.assertIn("unavailable", g.check("node.log", 60, 0, 1000))
        self.assertIn("future", self.check(g, info(mtime=1001), 0, 1000))

    def test_monotonic_clock_after_initial_sample(self):
        g = watch.LogGrowth()
        self.assertIsNone(self.check(g, info(), 0, 1000))
        self.assertIsNone(self.check(g, info(), 30, 100000))
        self.assertIn("not grown", self.check(g, info(), 61, 100000))

    def test_independent_log_and_status_failure(self):
        args = SimpleNamespace(mina="mina", url="http://localhost:3000", max_tip_age=1800,
                               min_peers=1, timeout=1, status_only=True, log_file=Path("missing"), max_log_silence=15)
        with patch.object(watch.os, "stat", side_effect=FileNotFoundError(2, "missing")), \
             patch.object(watch.subprocess, "run", return_value=SimpleNamespace(returncode=1, stdout="UNHEALTHY: tip old\n", stderr="")):
            self.assertEqual(len(watch.sample(args, watch.LogGrowth())), 2)
            self.assertIn("--status-only", watch.subprocess.run.call_args.args[0])
        args.log_file = None
        with patch.object(watch.subprocess, "run", side_effect=subprocess.TimeoutExpired("mina", 8)):
            self.assertIn("failed", watch.sample(args, watch.LogGrowth())[0])


if __name__ == "__main__":
    unittest.main()
