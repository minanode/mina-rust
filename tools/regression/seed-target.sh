#!/usr/bin/env bash
# Safely seed a Cargo target; see --help for the quiescence contract.
set -euo pipefail
exec python3 "$(dirname "${BASH_SOURCE[0]}")/seed-target.py" "$@"
