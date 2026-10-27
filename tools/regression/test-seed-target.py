#!/usr/bin/env python3
"""Exercise hard-link isolation and refusal paths without a Cargo compile."""
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("seed_target", HERE / "seed-target.py")
seed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(seed)


class Isolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="t1-seed-test-")
        self.root = Path(self.temp.name)
        self.donor = self.root / "donor" / "target"
        self.dest = self.root / "dest" / "target"
        self.dest.parent.mkdir()
        self.contents = {
            "CACHEDIR.TAG": b"test target", ".rustc_info.json": b"{}",
            "release/.cargo-lock": b"", "debug/.cargo-lock": b"",
            "release/build/probe-abc/output": b"cargo:rustc-env=SHA=original",
            "release/build/probe-abc/out/action_kind.rs": b"original action",
            "release/build/vendor-abc/out/libnative.a": b"mutable archive",
            "release/build/probe-0123456789abcdef/build-script-build": b"shared compiled code",
            "release/build/probe-0123456789abcdef/out/build-script-build": b"mutable generated code",
            "release/.fingerprint/probe-abc/lib-probe": b"original fingerprint",
            "release/deps/libvendor-abc.rlib": b"shared artifact",
            "release/deps/libprobe-abc.rmeta": b"compiler metadata",
            "release/deps/probe-abc.d": b"old dep info",
            "release/incremental/probe-abc/cache": b"incremental cache",
            "wasm/wasm32-unknown-unknown/release/build/probe-abc/output": b"wasm",
        }
        for rel, content in self.contents.items():
            p = self.donor / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(content)
            os.utime(p, ns=(1234567890000000000, 1234567890000000000))

    def tearDown(self):
        self.temp.cleanup()

    def run_seed(self, *args, expected=0):
        p = subprocess.run([str(HERE / "seed-target.sh"), *map(str, args)],
                           capture_output=True, text=True,
                           env={**os.environ, "MINA_LOCK_DIR": str(self.root / "locks")})
        self.assertEqual(p.returncode, expected, p.stdout + p.stderr)
        return json.loads(p.stdout) if p.stdout else None

    def assert_isolated(self):
        for rel, content in self.contents.items():
            a, b = self.donor / rel, self.dest / rel
            self.assertEqual(a.read_bytes(), content)
            if a.name == ".cargo-lock" or "incremental" in Path(rel).parts:
                self.assertFalse(b.exists())
                continue
            self.assertEqual(b.read_bytes(), content)
            self.assertEqual(a.stat().st_mtime_ns, b.stat().st_mtime_ns)
            self.assertEqual(a.stat().st_mode, b.stat().st_mode)
            if seed.mutable(Path(rel)):
                self.assertNotEqual(a.stat().st_ino, b.stat().st_ino)
                self.assertEqual(b.stat().st_nlink, 1)
                b.write_bytes(b"destination only")
                self.assertEqual(a.read_bytes(), content)
            else:
                self.assertEqual(a.stat().st_ino, b.stat().st_ino)

    def test_seed_content_metadata_and_mutations(self):
        report = self.run_seed(self.donor, self.dest)
        self.assertGreater(report["copied_bytes"], 0)
        self.assertGreater(report["shared_artifact_bytes"], 0)
        self.assert_isolated()

    def test_repair_and_idempotency(self):
        subprocess.run(["cp", "-al", str(self.donor), str(self.dest)], check=True)
        self.run_seed("--repair", self.dest)
        report = self.run_seed("--repair", self.dest)
        self.assertEqual(report["private_files"], 0)
        self.assertEqual(report["removed_locks"], [])
        self.assert_isolated()

    def test_repair_generated_source_renamed_from_raw_seed(self):
        subprocess.run(["cp", "-al", str(self.donor), str(self.dest)], check=True)
        relative = "release/build/probe-abc/out/action_kind.rs"
        donor_output = self.donor / relative
        source = self.dest.parent / "src/action_kind.rs"
        source.parent.mkdir()
        source.write_bytes(b"previous generated source")
        subprocess.run(["git", "init", "-q", str(self.dest.parent)], check=True)
        subprocess.run(["git", "-C", str(self.dest.parent), "add", str(source)], check=True)
        # The old node build script moved the raw-seeded OUT_DIR inode into
        # tracked source, leaving no alias inside the destination target.
        os.replace(self.dest / relative, source)
        before = source.stat()
        self.assertEqual(before.st_ino, donor_output.stat().st_ino)
        self.assertEqual(before.st_nlink, 2)
        report = self.run_seed("--repair", self.dest)
        self.assertEqual(report["status"], "isolated")
        self.assertEqual(source.stat().st_nlink, 1)
        self.assertEqual(report["private_source_files"], 1)
        self.assertNotEqual(source.stat().st_ino, before.st_ino)
        self.assertEqual(source.stat().st_mode, before.st_mode)
        self.assertEqual(source.stat().st_mtime_ns, before.st_mtime_ns)
        donor_output.write_bytes(b"donor changed after repair")
        self.assertEqual(source.read_bytes(), b"original action")
        self.assertEqual(self.run_seed("--repair", self.dest)["private_source_files"], 0)

    def test_seed_isolates_external_tracked_and_ignored_sources(self):
        subprocess.run(["git", "init", "-q", str(self.dest.parent)], check=True)
        (self.dest.parent / ".gitignore").write_text("generated/\n")
        tracked = self.dest.parent / "src/action_kind.rs"
        tracked.parent.mkdir()
        tracked.write_bytes(b"tracked generated source")
        subprocess.run(["git", "-C", str(self.dest.parent), "add", str(tracked)], check=True)
        os.link(tracked, self.root / "outside-tracked")
        generated = self.dest.parent / "generated/extensionless"
        generated.parent.mkdir()
        external = self.root / "outside-generated"
        external.write_bytes(b"ignored generated source")
        os.link(external, generated)
        dry = self.run_seed("--dry-run", self.donor, self.dest)
        self.assertEqual(dry["planned_source_files"], 2)
        self.assertEqual(tracked.stat().st_nlink, 2)
        self.assertEqual(generated.stat().st_nlink, 2)
        report = self.run_seed(self.donor, self.dest)
        self.assertEqual(report["private_source_files"], 2)
        (self.root / "outside-tracked").write_bytes(b"external tracked write")
        external.write_bytes(b"external ignored write")
        self.assertEqual(tracked.read_bytes(), b"tracked generated source")
        self.assertEqual(generated.read_bytes(), b"ignored generated source")
        self.assertEqual(tracked.stat().st_nlink, 1)
        self.assertEqual(generated.stat().st_nlink, 1)

    def test_repair_source_alias_to_immutable_target_artifact(self):
        self.run_seed(self.donor, self.dest)
        artifact = self.dest / "release/deps/libvendor-abc.rlib"
        source = self.dest.parent / "untracked-source"
        os.link(artifact, source)
        self.run_seed("--repair", self.dest)
        artifact.write_bytes(b"artifact changed")
        self.assertEqual(source.read_bytes(), b"shared artifact")
        self.assertEqual(source.stat().st_nlink, 1)

    def test_repair_finds_sources_above_nested_target(self):
        subprocess.run(["git", "init", "-q", str(self.dest.parent)], check=True)
        nested = self.dest / "wasm"
        self.dest.mkdir()
        self.run_seed(self.donor, nested)
        source = self.dest.parent / "generated.rs"
        external = self.root / "external.rs"
        external.write_bytes(b"generated source above target parent")
        os.link(external, source)
        self.run_seed("--repair", nested)
        external.write_bytes(b"outside write")
        self.assertEqual(source.read_bytes(), b"generated source above target parent")

    def test_source_scan_does_not_enter_separate_checkout_or_symlink(self):
        separate = self.dest.parent / ".claude/worktrees/other"
        separate.mkdir(parents=True)
        (separate / ".git").write_text("gitdir: elsewhere\n")
        external = self.root / "external.rs"
        external.write_bytes(b"other checkout source")
        os.link(external, separate / "source.rs")
        (self.dest.parent / "symlink").symlink_to(self.root, target_is_directory=True)
        self.run_seed(self.donor, self.dest)
        self.assertEqual(external.stat().st_nlink, 2)

    def test_refuses_new_source_link_before_success(self):
        self.run_seed(self.donor, self.dest)
        report = {"removed_locks": []}
        with unittest.mock.patch.object(seed, "source_links", side_effect=[[], [Path("late.rs")]]):
            with self.assertRaisesRegex(seed.Busy, "source links remain"):
                seed.isolate(self.dest, [], report, set())

    def test_busy_cargo_lock(self):
        with (self.donor / "release/.cargo-lock").open("rb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            self.run_seed(self.donor, self.dest, expected=75)
        self.assertFalse(self.dest.exists())

    def test_busy_regression_slot(self):
        locks = self.root / "locks"
        locks.mkdir()
        subprocess.run(["cp", "-al", str(self.donor), str(self.dest)], check=True)
        with (locks / "build-2.lock").open("ab") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            self.run_seed("--repair", self.dest, expected=75)
        self.assertEqual((self.donor / ".rustc_info.json").stat().st_ino,
                         (self.dest / ".rustc_info.json").stat().st_ino)

    def test_running_executable(self):
        exe = self.donor / "release/sleep"
        shutil.copy2("/usr/bin/sleep", exe)
        p = subprocess.Popen([str(exe), "30"], cwd=self.donor.parent)
        try:
            self.run_seed(self.donor, self.dest, expected=75)
            self.assertIsNone(p.poll())
        finally:
            p.terminate()  # Only the process created by this test.
            p.wait()
        self.assertFalse(self.dest.exists())

    def test_refuse_existing_destination_and_symlinks(self):
        self.dest.mkdir()
        self.run_seed(self.donor, self.dest, expected=2)
        self.dest.rmdir()
        (self.donor / "outside").symlink_to(self.root)
        self.run_seed(self.donor, self.dest, expected=2)
        self.assertFalse(self.dest.exists())

    def test_dry_run_does_not_write(self):
        self.run_seed("--dry-run", self.donor, self.dest)
        self.assertFalse(self.dest.exists())

    def test_workspace_fingerprints_are_invalidated(self):
        (self.dest.parent / "Cargo.toml").write_text(
            '[package]\nname="probe"\nversion="0.1.0"\n')
        marker = self.donor / "release/.fingerprint/probe-0123456789abcdef/lib-probe"
        marker.parent.mkdir()
        marker.write_bytes(b"untrustworthy workspace cache")
        report = self.run_seed(self.donor, self.dest)
        self.assertEqual(report["invalidated_workspace_files"], 1)
        self.assertFalse((self.dest / marker.relative_to(self.donor)).exists())
        self.assertEqual(marker.read_bytes(), b"untrustworthy workspace cache")

    def test_floor(self):
        with unittest.mock.patch.object(seed.shutil, "disk_usage") as usage:
            usage.return_value.free = seed.FLOOR - 1
            with self.assertRaises(OSError):
                seed.free_check(self.root)


if __name__ == "__main__":
    import unittest.mock
    unittest.main()
