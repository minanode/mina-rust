#!/usr/bin/env bash
# Live Mesa restore gates. See restart.py --help and I5d2b-restore-gates.md.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
exec python3 "$here/restart.py" "$@"
