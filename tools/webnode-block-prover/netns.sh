#!/usr/bin/env bash
# Run a command in a new unprivileged user and network namespace whose only
# interface is loopback, with no route: nothing in it can reach a peer on
# any network, public or local. No sudo needed. The I34c browser runs use
# it around prepare.py and run.mjs (see private implementation notes):
#
#   tools/webnode-block-prover/netns.sh COMMAND [ARGS...]
#
# The namespace ends with COMMAND. Its interfaces and routes are printed to
# stderr first, as evidence.
set -euo pipefail
exec unshare --user --map-root-user --net bash -c '
  set -euo pipefail
  ip link set lo up
  if [[ -n $(ip route show table main) || -n $(ip -6 route show table main | grep -v "^::1 ") ]]; then
    echo "netns.sh: the namespace has a route; refusing to run" >&2
    exit 1
  fi
  { echo "netns.sh: interfaces:"; ip -br addr; echo "netns.sh: no routes"; } >&2
  exec "$@"
' netns "$@"
