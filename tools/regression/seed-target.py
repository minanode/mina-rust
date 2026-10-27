#!/usr/bin/env python3
"""Hard-link artifacts, isolate mutable Cargo state, and report allocated bytes."""

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import re
import tomllib
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import time

FLOOR = 55 * 1024**3  # Use GiB: stricter than the requested 55 GB floor.
BUILDERS = {"cargo", "rustc", "rustdoc", "rustfmt", "cc", "c++", "gcc", "g++",
            "clang", "clang++", "ld", "ld.lld", "rust-lld", "collect2", "ar",
            "make", "gmake", "cmake", "ninja", "wasm-pack", "wasm-bindgen"}


class Busy(Exception):
    pass


def under(path, root):
    return path == str(root) or path.startswith(str(root) + os.sep)


def busy_processes(target):
    """Never read environments/arguments; report only PID, comm and reason."""
    root = worktree_root(target)
    busy = []
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit() or int(proc.name) == os.getpid():
            continue
        comm = ""
        try:
            comm = (proc / "comm").read_text().strip()
            cwd = os.readlink(proc / "cwd")
            exe = os.readlink(proc / "exe")
            builder = comm in BUILDERS or comm.startswith(("build-script-", "rustc", "cargo"))
            reason = None
            if under(exe, target):
                reason = "executable in target"
            elif under(cwd, target) or (builder and under(cwd, root)):
                reason = "build cwd in worktree"
            if reason is None:
                for fd in (proc / "fd").iterdir():
                    try:
                        opened = os.readlink(fd)
                    except FileNotFoundError:
                        continue
                    if under(opened, target) or (builder and under(opened, root)):
                        reason = "open file in target/worktree"
                        break
            if reason:
                busy.append({"pid": int(proc.name), "comm": comm, "reason": reason})
        except (FileNotFoundError, ProcessLookupError):
            continue
        except PermissionError:
            # Same-user builders must be inspectable. System services (including
            # the user systemd) may be nondumpable; they are not compilers.
            if proc.stat().st_uid == os.getuid() and (comm in BUILDERS or comm.startswith(("build-script-", "rustc", "cargo"))):
                raise Busy(f"cannot inspect same-user process {proc.name}")
    return busy


def files(target):
    for base, dirs, names in os.walk(target, followlinks=False):
        for name in dirs + names:
            path = Path(base) / name
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode) or not (stat.S_ISDIR(mode) or stat.S_ISREG(mode)):
                raise ValueError(f"unsupported symlink/special entry: {path}")
        for name in names:
            yield Path(base) / name


def worktree_root(target):
    # A nested target (e.g. target/wasm) still belongs to the whole checkout.
    # Standalone scratch targets have no Git metadata: use their parent.
    for root in target.parents:
        if (root / ".git").exists():
            return root
    return target.parent


def source_files(target):
    """All regular checkout files outside this target, including ignored output.

    Never follow symlinks or enter Git metadata or a separate nested checkout.
    Do not filter by extensions or Git's ignore rules: generated sources need
    not be tracked, named action_kind.rs, or even have a source extension.
    """
    root = worktree_root(target)

    def walk_error(error):
        raise error

    for base, dirs, names in os.walk(root, followlinks=False, onerror=walk_error):
        dirs[:] = [name for name in dirs
                   if name != ".git" and Path(base) / name != target
                   and not (Path(base) / name).is_symlink()
                   and not (Path(base) / name / ".git").exists()]
        for name in names:
            path = Path(base) / name
            if name != ".git" and stat.S_ISREG(path.lstat().st_mode):
                yield path


def source_links(target):
    # nlink > 1 catches aliases outside the checkout without needing to locate
    # the other name, including OUT_DIR files already renamed out of target.
    # Also detach entirely internal aliases: no source should share target's
    # immutable artifacts, and a private inode is easy to verify afterwards.
    return [path for path in source_files(target) if path.stat().st_nlink > 1]


def mutable(relative):
    # Cargo's compiled build-script executables are replaced just like deps
    # executables. This exception applies only directly under build/pkg-HASH,
    # never to OUT_DIR: build scripts may rewrite ANY file there in place.
    if (relative.parent.parent.name == "build"
            and re.fullmatch(r".+-[0-9a-f]{16}", relative.parent.name)
            and (relative.name == "build-script-build"
                 or re.fullmatch(r"build_script_build-[0-9a-f]{16}", relative.name))):
        return False
    if any(part in {"build", ".fingerprint", "incremental"} for part in relative.parts):
        return True
    # The compiler replaces these linked artifacts rather than truncating them.
    # All metadata, dep-info, locks and unknown/new artifact kinds are private.
    return not (relative.parent.name == "deps" and (
        relative.suffix in {".rlib", ".rmeta", ".so"}
        or re.fullmatch(r".+-[0-9a-f]{16}", relative.name)
    ))


def local_packages(target):
    for root in target.parents:
        manifest = root / "Cargo.toml"
        if not manifest.is_file():
            continue
        data = tomllib.loads(manifest.read_text())
        manifests = [manifest]
        for member in data.get("workspace", {}).get("members", []):
            manifests.extend(p / "Cargo.toml" for p in root.glob(member))
        packages, visited = set(), set()
        while manifests:
            current = manifests.pop().resolve()
            if current in visited or not current.is_file():
                continue
            visited.add(current)
            package = tomllib.loads(current.read_text())
            name = package.get("package", {}).get("name")
            if name:
                packages.add(name)
            # Cargo implicitly includes in-tree path dependencies, even when
            # they are absent from workspace.members (e.g. binprot_derive).
            tables = [package, package.get("workspace", {})]
            tables.extend(package.get("target", {}).values())
            for table in tables:
                for key in ("dependencies", "dev-dependencies", "build-dependencies"):
                    for dependency in table.get(key, {}).values():
                        if isinstance(dependency, dict) and "path" in dependency:
                            manifests.append(current.parent / dependency["path"] / "Cargo.toml")
        return packages
    return set()


def workspace_fingerprint(relative, packages):
    parts = relative.parts
    if ".fingerprint" not in parts:
        return False
    index = parts.index(".fingerprint") + 1
    if len(parts) <= index:
        return False
    package, _, suffix = parts[index].rpartition("-")
    return package in packages and re.fullmatch(r"[0-9a-f]{16}", suffix)


def free_check(path, needed=0):
    free = shutil.disk_usage(path).free
    if free - needed < FLOOR:
        raise OSError(f"disk floor: {free} free bytes, need {needed}, reserve {FLOOR}")
    return free


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").digest()


def private_copy(path):
    before = path.stat()
    free_check(path, before.st_size)
    fd, name = tempfile.mkstemp(prefix=".seed-target-", dir=path.parent)
    os.close(fd)
    temp = Path(name)
    try:
        shutil.copy2(path, temp)
        if digest(path) != digest(temp):
            raise Busy(f"content changed while copying {path}")
        after = path.stat()
        if (before.st_ino, before.st_size, before.st_mtime_ns) != (
                after.st_ino, after.st_size, after.st_mtime_ns):
            raise Busy(f"file changed while copying {path}")
        allocated = temp.stat().st_blocks * 512
        os.replace(temp, path)
        return before.st_size, allocated
    finally:
        temp.unlink(missing_ok=True)


@contextlib.contextmanager
def build_slots():
    """Exclude cooperating build launches while repairing existing lock names."""
    configured = os.environ.get("MINA_LOCK_DIR")
    if configured:
        directory = Path(configured)
    else:
        common = subprocess.check_output(
            ["git", "-C", str(Path(__file__).resolve().parent), "rev-parse",
             "--path-format=absolute", "--git-common-dir"], text=True).strip()
        directory = Path(common).parent / ".work/locks"
    directory.mkdir(parents=True, exist_ok=True)
    handles = []
    try:
        for number in range(1, 4):
            path = directory / f"build-{number}.lock"
            handle = path.open("ab")
            handles.append(handle)
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise Busy(f"regression build slot occupied: {path}") from None
        yield
    finally:
        for handle in handles:
            handle.close()


@contextlib.contextmanager
def locked(target, paths):
    """Hold existing Cargo lock inodes while inspecting/copying their target."""
    locks = []
    try:
        for path in paths:
            if path.name != ".cargo-lock":
                continue
            handle = path.open("rb")
            locks.append(handle)
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise Busy(f"Cargo lock held: {path}") from None
        busy = busy_processes(target)
        if busy:
            raise Busy(json.dumps(busy))
        yield
    finally:
        for handle in locks:
            handle.close()


def isolate(target, paths, report, packages):
    # Do this before removing target aliases: a generated source can outlive
    # its OUT_DIR name, or alias an otherwise immutable artifact in target.
    for path in source_links(target):
        size, blocks = private_copy(path)
        report["private_source_files"] += 1
        report["private_files"] += 1
        report["copied_bytes"] += size
        report["allocated_bytes"] += blocks
    # Lock entries are removed last. The caller must exclude new builds for
    # the operation: /proc is a point-in-time guard, not a process-launch lock.
    for path in paths:
        if path.name == ".cargo-lock":
            continue
        relative = path.relative_to(target)
        if "incremental" in relative.parts:
            report["discarded_incremental_bytes"] += path.stat().st_size
            path.unlink()
            report["discarded_incremental_files"] += 1
            continue
        if workspace_fingerprint(relative, packages):
            path.unlink()
            report["invalidated_workspace_files"] += 1
            continue
        if mutable(relative) and path.stat().st_nlink > 1:
            size, blocks = private_copy(path)
            report["private_files"] += 1
            report["copied_bytes"] += size
            report["allocated_bytes"] += blocks
    busy = busy_processes(target)
    if busy:
        raise Busy(json.dumps(busy))
    for path in paths:
        if path.name == ".cargo-lock":
            path.unlink()
            report["removed_locks"].append(str(path.relative_to(target)))
    remaining = [str(p.relative_to(target)) for p in files(target)
                 if mutable(p.relative_to(target)) and p.stat().st_nlink > 1]
    if remaining:
        raise Busy(f"mutable links appeared during isolation: {remaining[:5]}")
    remaining = source_links(target)
    if remaining:
        raise Busy(f"source links remain after isolation: {remaining[:5]}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, epilog="""
Donor and destination must stay idle until this command exits. On this shared
machine, --repair takes all three regression build-slot locks (nonblocking)
and checks /proc before changing an existing target. This tool never signals a process. Seed requires
an absent destination on the same filesystem. --repair preserves file bytes
and timestamps of retained files; workspace fingerprint entries and locks are
unlinked so the next build cannot trust transplanted workspace metadata.
Both modes also privately copy all multiply-linked regular checkout files
outside TARGET, including tracked and ignored generated sources. Git metadata,
symlinks and separate nested checkouts are not traversed. The checkout root is
the nearest ancestor with .git, or TARGET's parent for standalone scratch trees.
Incremental session files are discarded rather than copying their locks/caches. Busy targets
exit 75, other failures exit 2; partial repairs are safe to retry. JSON on stdout
reports actual copied/allocated bytes, separate from shared artifact bytes.
""")
    parser.add_argument("--repair", action="store_true", help="isolate an existing target in place")
    parser.add_argument("--dry-run", action="store_true", help="inspect only; no copies or unlinking")
    parser.add_argument("paths", nargs="+")
    args = parser.parse_args()
    if len(args.paths) != (1 if args.repair else 2):
        parser.error("use DONOR_TARGET DEST_TARGET, or --repair TARGET")
    donor = Path(args.paths[0]).absolute()
    if donor.is_symlink() or donor.resolve() != donor:
        raise ValueError("source/repair target must be a canonical path, without symlinks")
    if not donor.is_dir() or not ((donor / "CACHEDIR.TAG").exists()
                                  or (donor / ".rustc_info.json").exists()):
        raise ValueError(f"not a Cargo target directory: {donor}")
    target = donor if args.repair else Path(args.paths[1]).absolute()
    if not args.repair:
        if target.exists() or target.is_symlink():
            raise ValueError(f"destination must not exist: {target}")
        if target.parent.resolve() != target.parent or not target.parent.is_dir():
            raise ValueError("destination parent must exist and be canonical")
        if under(str(target), donor) or target.parent.stat().st_dev != donor.stat().st_dev:
            raise ValueError("targets must be separate and on the same filesystem")
        if under(str(donor), worktree_root(target)):
            raise ValueError("donor must be outside the destination checkout")
    packages = local_packages(target) or local_packages(donor)
    paths = list(files(donor))  # Validate the whole tree before any mutation.
    report = {"target": str(target), "donor": str(donor), "repair": args.repair,
              "dry_run": args.dry_run, "private_files": 0, "private_source_files": 0,
              "source_root": str(worktree_root(target)), "copied_bytes": 0,
              "allocated_bytes": 0, "removed_locks": [], "invalidated_workspace_files": 0,
              "workspace_packages": sorted(packages), "discarded_incremental_files": 0,
              "discarded_incremental_bytes": 0,
              "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    planned = [p for p in paths if p.name != ".cargo-lock" and
               mutable(p.relative_to(donor)) and "incremental" not in p.relative_to(donor).parts and not workspace_fingerprint(p.relative_to(donor), packages) and (not args.repair or p.stat().st_nlink > 1)]
    sources = source_links(target)
    report["planned_source_files"] = len(sources)
    report["planned_files"] = len(planned) + len(sources)
    report["planned_bytes"] = sum(p.stat().st_size for p in planned + sources)
    report["shared_artifact_bytes"] = sum(p.stat().st_size for p in paths
                                          if not mutable(p.relative_to(donor)))
    started = time.monotonic()
    try:
        with contextlib.ExitStack() as stack:
            if args.repair:
                stack.enter_context(build_slots())
            stack.enter_context(locked(donor, paths))
            if not args.repair:
                stack.enter_context(locked(target, []))
            report["free_before_bytes"] = free_check(target.parent, report["planned_bytes"])
            if not args.dry_run:
                if not args.repair:
                    # cp -al is confined to our idle donor, then mutable files
                    # are replaced in the destination before it may be used.
                    subprocess.run(["cp", "-al", "--", str(donor), str(target)], check=True)
                    paths = list(files(target))
                isolate(target, paths, report, packages)
            report["directory_bytes"] = (sum(Path(base).stat().st_blocks * 512
                for base, _, _ in os.walk(target)) if not args.repair and not args.dry_run else 0)
            report["new_allocated_bytes"] = report["allocated_bytes"] + report["directory_bytes"]
            report["free_after_bytes"] = shutil.disk_usage(target.parent).free
            report["status"] = "inspected" if args.dry_run else "isolated"
    except (Busy, OSError, ValueError, subprocess.SubprocessError) as error:
        report["status"] = "skipped_or_partial"
        report["error"] = str(error)
        raise
    finally:
        report["seconds"] = round(time.monotonic() - started, 3)
        print(json.dumps(report, sort_keys=True), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Busy as error:
        print(f"seed-target: busy: {error}", file=sys.stderr)
        sys.exit(75)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"seed-target: {error}", file=sys.stderr)
        sys.exit(2)
