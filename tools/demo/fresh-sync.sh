#!/usr/bin/env bash
# Sync from an empty directory with the installed release binary and dashboard.
# Keep this file beside sync-run.py. Requires Python 3; no Python packages.
#
#   ./fresh-sync.sh --network devnet --binary ~/.local/bin/mina --keep
#   ./fresh-sync.sh --docker ghcr.io/minanode/mina-rust:0.20.0 --keep
#   ./fresh-sync.sh --port 4060 --libp2p-port 9260 --output demo-runs/run-1
#
# --keep waits for Ctrl-C after syncing. Every exit stops only this node and
# deletes its temporary data. --output retains logs and measurements, not keys.
# Use --help for timeouts, disk limits and all options.
set -euo pipefail
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
exec python3 "$here/sync-run.py" "$@"
