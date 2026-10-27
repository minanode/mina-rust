#!/usr/bin/env bash
# Usage: check-build-env.sh BINARY EXPECTED_COMMIT
# Read the same BuildEnv as /build_env without starting a node. EXPECTED_COMMIT
# is a full SHA-1 or a commit/ref resolvable in the caller's git repository.
# Exit 0: match; 1: mismatch; 2: unreadable/ambiguous/changed binary or identity.
set -euo pipefail
exec python3 - "$@" <<'PY'
import hashlib
import json
import pathlib
import re
import subprocess
import sys


def main():
    if len(sys.argv) != 3:
        raise ValueError("usage: check-build-env.sh BINARY EXPECTED_COMMIT")
    binary = pathlib.Path(sys.argv[1]).resolve(strict=True)
    expected = sys.argv[2]
    if not re.fullmatch(r"[0-9a-f]{40}", expected):
        expected = subprocess.check_output(
            ["git", "rev-parse", "--verify", "--end-of-options", expected + "^{commit}"],
            text=True, stderr=subprocess.PIPE,
        ).strip()
    if not re.fullmatch(r"[0-9a-f]{40}", expected):
        raise ValueError("expected commit must resolve to one full SHA-1")
    before = binary.stat()
    result = subprocess.run(
        [str(binary), "build-info"], text=True, capture_output=True,
        timeout=30, check=True,
    )
    commits = re.findall(r"^Commit SHA:\s*([0-9a-f]{40})\s*$", result.stdout, re.M)
    if len(commits) != 1:
        raise ValueError("build-info did not return exactly one full Commit SHA")
    with binary.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    after = binary.stat()
    identity = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
    if identity(before) != identity(after):
        raise ValueError("binary changed during provenance check; retry when idle")
    matches = commits[0] == expected
    print(json.dumps({"binary": str(binary), "sha256": digest,
                      "expected_commit": expected, "actual_commit": commits[0],
                      "matches": matches}, sort_keys=True))
    return 0 if matches else 1


try:
    sys.exit(main())
except (OSError, ValueError, subprocess.SubprocessError) as error:
    print(f"check-build-env: {error}", file=sys.stderr)
    sys.exit(2)
PY
