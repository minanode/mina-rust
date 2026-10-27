#!/usr/bin/env bash
set -euo pipefail
repo=$PWD
cd crates/web-verifier/tests/browser
npm ci
export PUPPETEER_DIR=$PWD
export CHROME=$(node install-browser.mjs "$PWD/.browser")
cd "$repo"
BASE=${BASE:-http://127.0.0.1:4211} OUT=${OUT:-$PWD/browser-results} \
    UNTIL_SYNCED=60 LOCK_WAIT=120 node tools/regression/webnode/run.mjs 15
