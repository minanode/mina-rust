#!/usr/bin/env bash
# Mesa's network config is process-global: never combine these two filters.
set -euo pipefail
g=${GATE_DIR:?set GATE_DIR to the test build output}
: "${MINA_CIRCUIT_BLOBS_BASE_DIR:?set the directory containing both 4.0.2 network blob directories}"
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
exe=$(python3 - "$g" <<'PY'
import json, pathlib, sys
g = pathlib.Path(sys.argv[1])
metadata = json.loads((g / 'metadata.json').read_text())
ids = {p['id'] for p in metadata['packages'] if p['name'] == 'mina-tree'}
for line in (g / 'test-build.jsonl').read_text().splitlines():
    a = json.loads(line)
    if (a.get('reason') == 'compiler-artifact' and a.get('package_id') in ids
            and a.get('executable') and 'lib' in a['target']['kind']
            and a.get('profile', {}).get('test')):
        print(g / 'tests/bin' / pathlib.Path(a['executable']).name)
        break
else:
    raise SystemExit('no mina-tree library test executable')
PY
)
mkdir -p "$g/tests/prover"
cd "$repo/crates/ledger"
export RUST_MIN_STACK=16777216 RAYON_NUM_THREADS=8
"$exe" --test-threads=1 --ignored mesa_devnet >"$g/tests/prover/devnet.log" 2>&1
"$exe" --test-threads=1 --ignored --skip mesa_devnet mesa_ >"$g/tests/prover/mainnet.log" 2>&1
python3 - "$g/tests/prover" <<'PY'
import json, pathlib, re, sys
out = pathlib.Path(sys.argv[1])
rows = []
for network, count in [('devnet', 7), ('mainnet', 19)]:
    text = (out / (network + '.log')).read_text()
    matches = re.findall(r'test result: ok\. (\d+) passed; 0 failed;', text)
    if matches != [str(count)]:
        raise SystemExit(f'{network}: expected {count} passing prover tests, got {matches}')
    rows.append(dict(network=network, passed=count, failed=0))
(out / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')
print(json.dumps(rows))
PY
