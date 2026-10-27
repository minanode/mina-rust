#!/usr/bin/env bash
# m9's test builds, as at m8 gate a and wave 4b: the 11 packages' tests with
# libp2p and WebRTC, I18's four scenario targets, the prove_block example
# harness, the standalone binprot tests, then the release workspace check.
# JSON artefact lists go to the gates' directory. Run under locked.sh.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
g=${GATE_DIR:?set GATE_DIR to a fresh absolute output directory}
mkdir -p "$g"
export CARGO_BUILD_JOBS=8
if [[ ${MILESTONE_BUILD_LOCKED:-} != 1 ]]; then
  export MILESTONE_BUILD_LOCKED=1
  exec "$repo/tools/regression/lock.sh" build -- bash "$0"
fi
cd "$repo"
cargo metadata --format-version 1 >"$g/metadata.json"
start=$(date +%s)
cargo test --locked --release --no-run \
  -p mina-tree -p mina-core -p mina-node -p mina-p2p \
  -p mina-p2p-messages -p mina-block-replay -p mina-node-common \
  -p mina-node-native -p ledger-tool -p mina-cli -p mina-isolated-prover \
  --features mina-p2p/p2p-libp2p,mina-p2p/p2p-webrtc --message-format=json \
  >"$g/test-build.jsonl"
echo "test build wall=$(( $(date +%s) - start ))s"
start=$(date +%s)
cargo test --locked --release --no-run -p mina-node-testing \
  --test p2p_basic_incoming --test p2p_basic_connections \
  --test p2p_kad --test p2p_basic_outgoing --message-format=json \
  >"$g/scenarios-build.jsonl"
echo "scenarios build wall=$(( $(date +%s) - start ))s"
start=$(date +%s)
cargo test --locked --release --no-run \
  -p mina-tree -p mina-core -p mina-node -p mina-p2p \
  -p mina-p2p-messages -p mina-block-replay -p mina-node-common \
  -p mina-node-native -p ledger-tool -p mina-cli -p mina-isolated-prover \
  --features mina-p2p/p2p-libp2p,mina-p2p/p2p-webrtc --example prove_block \
  --message-format=json >"$g/proof-example-build.jsonl"
echo "prove_block example build wall=$(( $(date +%s) - start ))s"
start=$(date +%s)
cargo test --locked --release --no-run -p binprot --message-format=json >"$g/binprot-build.jsonl"
echo "binprot build wall=$(( $(date +%s) - start ))s"
git -C "$repo" rev-parse HEAD >"$g/test-build.commit"
start=$(date +%s)
cargo check --locked --release --workspace --all-targets
echo "check wall=$(( $(date +%s) - start ))s"
