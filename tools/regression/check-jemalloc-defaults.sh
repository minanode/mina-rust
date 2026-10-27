#!/usr/bin/env bash
# Check a native jemalloc Mina binary, including the stripped release artifact.
# Usage: check-jemalloc-defaults.sh BINARY
#
# An x86_64 binary runs directly. An aarch64 binary runs directly on an aarch64
# host, otherwise under qemu-user: set QEMU_AARCH64 (default qemu-aarch64-static
# on PATH) and ARM64_SYSROOT (a directory holding the ARM64 glibc loader and
# libraries, passed as qemu -L). On aarch64 the check also runs the binary
# under emulated 16 KiB and 64 KiB kernel pages (qemu -p).
set -euo pipefail
exec python3 - "$@" <<'PY'
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys

# jemalloc fixes its page size at build time and aborts on larger kernel pages.
# ARM64 builds use 64 KiB (see .cargo/config.toml) to cover 4, 16 and 64 KiB
# kernels; x86_64 kernels use 4 KiB pages.
EXPECTED_PAGE = {"x86_64": 4096, "aarch64": 65536}
ELF_MACHINE = {0x3E: "x86_64", 0xB7: "aarch64"}


def elf_arch(binary):
    header = binary.read_bytes()[:20]
    if header[:4] != b"\x7fELF" or header[4] != 2 or header[5] != 1:
        raise ValueError("expected a 64-bit little-endian ELF executable")
    machine = int.from_bytes(header[18:20], "little")
    if machine not in ELF_MACHINE:
        raise ValueError(f"unsupported ELF machine {machine:#x}")
    return ELF_MACHINE[machine]


def runner(arch):
    """Return a function building the command line for an emulated page size."""
    if platform.machine() == arch:
        return lambda page: None if page else []
    if arch != "aarch64":
        raise ValueError(f"cannot run a {arch} binary on {platform.machine()}")
    qemu = shutil.which(os.environ.get("QEMU_AARCH64", "qemu-aarch64-static"))
    sysroot = os.environ.get("ARM64_SYSROOT")
    if not qemu or not sysroot or not Path(sysroot, "lib/ld-linux-aarch64.so.1").exists():
        raise ValueError("an aarch64 binary needs QEMU_AARCH64 and ARM64_SYSROOT")
    return lambda page: [qemu, "-L", sysroot] + (["-p", str(page)] if page else [])


def run(prefix, binary, prefixed, unprefixed=None):
    env = dict(os.environ)
    env.pop("MALLOC_CONF", None)
    env["_RJEM_MALLOC_CONF"] = prefixed
    if unprefixed is not None:
        env["MALLOC_CONF"] = unprefixed
    result = subprocess.run(
        prefix + [str(binary), "build-info"], env=env, text=True,
        capture_output=True, timeout=120, check=True,
    )
    if not re.search(r"^Commit SHA:\s*[0-9a-f]{40}\s*$", result.stdout, re.M):
        raise ValueError("build-info did not return a commit identity")
    if "Unsupported system page size" in result.stderr:
        raise ValueError("jemalloc rejected the system page size")
    return result.stderr


def main():
    if len(sys.argv) != 2:
        raise ValueError("usage: check-jemalloc-defaults.sh BINARY")
    binary = Path(sys.argv[1]).resolve(strict=True)
    before = binary.stat()
    arch = elf_arch(binary)
    command = runner(arch)
    symbols = subprocess.check_output(
        ["nm", "--defined-only", str(binary)], text=True,
        stderr=subprocess.PIPE, timeout=30,
    )
    matches = re.findall(r"^\S+\s+([A-Za-z])\s+_rjem_malloc_conf$", symbols, re.M)
    # cargo-zigbuild release output can already be stripped. If any symbols
    # remain, require the strong global pointer; otherwise runtime checks below
    # still verify the compiled configuration and operator override behavior.
    if symbols.strip() and (len(matches) != 1 or matches[0] not in ("D", "R")):
        raise ValueError("_rjem_malloc_conf must be a strong data symbol")
    cases = [
        ("compiled default", "confirm_conf:true,abort_conf:true", None, "4"),
        ("prefixed override", "confirm_conf:true,abort_conf:true,narenas:8", None, "8"),
        ("unprefixed ignored", "confirm_conf:true,abort_conf:true", "narenas:1", "4"),
    ]
    checks = []
    for name, prefixed, unprefixed, expected in cases:
        stderr = run(command(None), binary, prefixed, unprefixed)
        program = re.findall(r'malloc_conf #2 [^\n]*: "([^"]*)"', stderr)
        if program != ["narenas:4"]:
            raise ValueError("unexpected program-global allocator configuration")
        # jemalloc's diagnostic labels say MALLOC_CONF even in prefixed builds.
        # The actual environment names are tested separately in these cases.
        arenas = re.findall(r"-- Set conf value: narenas:(\d+)", stderr)
        if not arenas or arenas[-1] != expected:
            raise ValueError("allocator option precedence check failed: " + name)
        checks.append(name)
    # The general statistics section reports the compiled page size.
    stderr = run(command(None), binary,
                 "abort_conf:true,stats_print:true,stats_print_opts:mdablxe")
    page = re.findall(r"^Page size: (\d+)$", stderr, re.M)
    if page != [str(EXPECTED_PAGE[arch])]:
        raise ValueError(f"jemalloc page size {page}, expected {EXPECTED_PAGE[arch]}")
    checks.append(f"page size {page[0]}")
    if arch == "aarch64":
        # ARM64 kernels can use 16 KiB or 64 KiB pages.
        for kernel_page in (16384, 65536):
            prefix = command(kernel_page)
            if prefix is None:
                checks.append(f"kernel page {kernel_page}: SKIP on native host")
                continue
            stderr = run(prefix, binary, "confirm_conf:true,abort_conf:true")
            arenas = re.findall(r"-- Set conf value: narenas:(\d+)", stderr)
            if not arenas or arenas[-1] != "4":
                raise ValueError(f"startup check failed on {kernel_page}-byte pages")
            checks.append(f"kernel page {kernel_page}")
    after = binary.stat()
    identity = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
    if identity(before) != identity(after):
        raise ValueError("binary changed during the check")
    print(json.dumps({"binary": str(binary), "arch": arch,
                      "symbol_type": matches[0] if matches else None,
                      "symbol_check": "PASS" if symbols.strip() else "SKIP: no symbols",
                      "checks": checks, "status": "PASS"}, sort_keys=True))


try:
    main()
except (OSError, ValueError, subprocess.SubprocessError) as error:
    print("check-jemalloc-defaults: " + str(error), file=sys.stderr)
    sys.exit(1)
PY
