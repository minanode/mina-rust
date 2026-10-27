#!/usr/bin/env bash
# The non-cargo checks of the test gate, as at wave 4b: the frontend unit
# tests (node --test), the soak watcher's tests, P1f's devnet watchdog tests
# with a private TMPDIR, and syntax checks of the scripts. Logs in aux/.
set -euo pipefail
g=${GATE_DIR:?set GATE_DIR to a fresh absolute output directory}
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
a=$g/aux
mkdir -p "$a" "$g/tmp-watch"
trap 'rm -rf "$g/tmp-watch"' EXIT
cd "$repo/frontend"
node --test tests/*.test.mjs >"$a/frontend.log" 2>&1
echo "exit=$?" >>"$a/frontend.log"
cd "$repo"
python3 -I tools/regression/test_watch_status.py >"$a/watcher.log" 2>&1
echo "exit=$?" >>"$a/watcher.log"
TMPDIR=$g/tmp-watch bash tools/regression/devnet/test-watch.sh >"$a/test-watch.log" 2>&1
echo "exit=$?" >>"$a/test-watch.log"
# Private evidence checks remain part of the private gate. The public export
# omits both checker and evidence; log that skip rather than losing the gate.
run_optional_private_check() {
  local script=$1 evidence=$2 log=$3
  if [[ -f "$script" ]]; then
    python3 -I "$script" >"$a/$log" 2>&1
    echo "exit=$?" >>"$a/$log"
  elif [[ -d "$evidence" ]]; then
    echo "FAIL: private evidence exists but its checker is missing: $script" >"$a/$log"
    return 1
  else
    echo "SKIP: private checker and evidence are not distributed in this checkout" >"$a/$log"
  fi
}
run_optional_private_check tools/regression/check_showcase_claims.py docs/showcase showcase-claims.log
run_optional_private_check docs/research/P4.6c-devnet/verify.py docs/research/P4.6c-devnet devnet-evidence.log
python3 -I tools/regression/milestone/test_aux_tests.py >"$a/aux-tests.log" 2>&1
echo "exit=$?" >>"$a/aux-tests.log"
syntax_failures=0
{
  for f in tools/regression/devnet/*.sh tools/regression/webnode/build.sh tools/local-mesa-net/*.sh \
    tools/webnode-block-prover/*.sh; do
    [ -f "$f" ] && { bash -n "$f" && echo "ok bash -n $f" || { echo "FAIL bash -n $f"; syntax_failures=$((syntax_failures + 1)); }; }
  done
  for f in tools/local-mesa-net/*.py tools/webnode-block-prover/*.py tools/regression/devnet/*.py \
    tools/p2p-error-census/*.py; do
    [ -f "$f" ] && { python3 -c 'import ast, sys; ast.parse(open(sys.argv[1]).read())' "$f" &&
      echo "ok parse $f" || { echo "FAIL parse $f"; syntax_failures=$((syntax_failures + 1)); }; }
  done
  for f in tools/regression/webnode/*.mjs tools/webnode-block-prover/*.mjs; do
    [ -f "$f" ] && { node --check "$f" && echo "ok node --check $f" || { echo "FAIL node --check $f"; syntax_failures=$((syntax_failures + 1)); }; }
  done
} >"$a/syntax.log" 2>&1
if ((syntax_failures)); then
  echo "FAILED: $syntax_failures syntax checks (see $a/syntax.log)" >&2
  exit 1
fi
rm -rf "$g/tmp-watch"
echo "done $(date -u +%FT%TZ)" >"$a/done"
