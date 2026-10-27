#!/usr/bin/env python3
"""Wave 4b tests: the m8 gates' run-tests.py (wave 4a's suite selection) on
the pinned wave 4b executables, plus focused runs for the merged inputs; never
runs the CLI bootstrap tests.

Usage: python3 -I run-tests.py MODE
MODE: ordinary, ignored, corpus, persist, i5b, writers, pool, producer,
example, binprot, scenario-<target>, r26 (mina-node verification:: and
mina-node-native http_server::), i5c (mina-node ledger:: and
transition_frontier::persist, mina-tree sparse_ledger::).
Results go to GATE_DIR/tests/MODE/. INPUT_DIR holds the replay fixtures. mina-p2p and mina-node-testing
executables run in a private user and network namespace with only loopback.
"""
import json, os, pathlib, re, subprocess, sys, time

g = pathlib.Path(os.environ['GATE_DIR']).resolve()
import argparse
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('mode')
parser.add_argument('--plan', action='store_true', help='print selected commands without executing them')
args = parser.parse_args()
mode = args.mode
manifest = g / ('scenarios-build.jsonl' if mode.startswith('scenario-')
                else 'binprot-build.jsonl' if mode == 'binprot'
                else 'proof-example-build.jsonl' if mode == 'example' else 'test-build.jsonl')
meta = json.loads((g / 'metadata.json').read_text())
packages = {p['id']: p for p in meta['packages']}
artifacts = {}
finished = False
for line in manifest.read_text().splitlines():
    try:
        a = json.loads(line)
    except ValueError:
        continue
    if a.get('reason') == 'build-finished':
        finished = a['success']
    if a.get('reason') == 'compiler-artifact' and a.get('executable') and a.get('profile', {}).get('test'):
        artifacts[(a['package_id'], a['target']['name'])] = a
assert finished, 'build not successful'
out = g / 'tests' / mode
out.mkdir(parents=True, exist_ok=True)
assert args.plan or not (out / 'results.jsonl').exists(), 'use fresh results directory'
shared = pathlib.Path(os.environ['INPUT_DIR']).resolve()
env = dict(os.environ, RUST_MIN_STACK='16777216', RAYON_NUM_THREADS='8',
           MINA_STATUS_TEST_BIN=str(g / 'tests/bin/mina'))
for v in ['P6C_FROM', 'P6C_TO']:
    env.pop(v, None)
order = ['mina-node', 'mina-node-common', 'mina-node-native', 'mina-p2p', 'mina-core', 'mina-tree',
         'mina-p2p-messages', 'mina-block-replay', 'ledger-tool', 'mina-cli', 'mina-isolated-prover',
         'mina-node-testing', 'binprot']
selected = []
for (pid, name), a in artifacts.items():
    p = packages[pid]
    pkg = p['name']
    kind = a['target']['kind']
    extra = []
    envextra = {}
    if mode.startswith('scenario-'):
        if name != mode.removeprefix('scenario-'):
            continue
    elif mode == 'example':
        if name != 'prove_block':
            continue
    elif mode == 'writers':
        if name not in ['gossip_decode', 'mina_node_native']:
            continue
        extra = ['--ignored', '--nocapture', 'write_level2']
        envextra = {'P6_PROBE_OUT': str(g / 'tests/writer-output' / name), 'P6_PROBE_NONCE': '0'}
    elif mode == 'persist':
        if pkg != 'mina-node' or 'lib' not in kind:
            continue
        extra = ['persist::', '--nocapture']
    elif mode == 'i5b':
        if pkg != 'mina-node' or 'lib' not in kind:
            continue
        extra = ['--ignored', '--nocapture', 'persisted_root_staged_ledger_matches_reconstruct']
        envextra = {'MINA_RECONSTRUCT_DUMP': str(shared / 'reconstruct-current.binprot'),
                    'I5B_STORE_DIR': str(g / 'tests/i5b-store')}
    elif mode == 'pool':
        if pkg != 'mina-tree' or 'lib' not in kind:
            continue
        extra = ['transaction_pool::ocaml_tests', '--nocapture']
    elif mode == 'producer':
        if pkg == 'mina-node' and 'lib' in kind:
            extra = ['block_producer::']
        elif pkg == 'mina-node-common' and 'lib' in kind:
            extra = ['service::block_producer::']
        else:
            continue
    elif mode == 'r26':
        if pkg == 'mina-node' and 'lib' in kind:
            extra = ['verification::']
        elif pkg == 'mina-node-native' and 'lib' in kind:
            extra = ['http_server::']
        else:
            continue
    elif mode == 'i5c':
        if pkg == 'mina-node' and 'lib' in kind:
            extra = ['ledger::', 'transition_frontier::persist']
        elif pkg == 'mina-tree' and 'lib' in kind:
            extra = ['sparse_ledger::']
        else:
            continue
    elif mode == 'ordinary':
        if pkg == 'mina-cli' and 'test' in kind and name != 'status':
            continue
        if pkg == 'mina-tree' and not ('lib' in kind or name in ['ocaml_mainnet_constants', 'wire_conversion_no_panic']):
            continue
        if pkg == 'mina-node' and not ('lib' in kind or name in ['ocaml_mainnet_constants', 'devnet_mesa_genesis',
                                                                  'runtime_fork_config', 'runtime_mainnet_genesis']):
            continue
    elif mode in ['ignored', 'corpus']:
        if mode == 'corpus' and name != 'user_command_corpus':
            continue
        if mode == 'ignored' and name == 'user_command_corpus':
            continue
        if pkg == 'mina-tree':
            continue  # prover-tests.sh uses a separate process per network
        elif pkg == 'mina-block-replay' and name in ['replay_subset', 'devnet_replay_subset', 'user_command_corpus']:
            extra = ['--ignored', '--nocapture']
            envextra = {'MINA_REPLAY_DIR': str(shared / 'replay'),
                        'MINA_GENESIS_BIN': str(shared / 'genesis-mainnet/mesa-mainnet.bin'),
                        'MINA_DEVNET_REPLAY_DIR': str(shared / 'replay-devnet'),
                        'MINA_DEVNET_GENESIS_BIN': str(shared / 'genesis-devnet/mesa-devnet.bin')}
        else:
            continue
    elif mode == 'binprot':
        assert pkg == 'binprot'
    else:
        raise ValueError(mode)
    assert pkg in order, pkg
    assert not (pkg == 'mina-cli' and 'test' in kind and name != 'status'), 'CLI integration not allowlisted'
    selected.append((p, a, extra, envextra))
selected.sort(key=lambda x: (order.index(x[0]['name']), x[1]['target']['name']))
print('SELECTED', [(p['name'], a['target']['name'], extra) for p, a, extra, e in selected], flush=True)
failed = []
for p, a, extra, envextra in selected:
    name = p['name'] + '--' + a['target']['name']
    cwd = pathlib.Path(p['manifest_path']).parent
    command = [str(g / 'tests/bin' / pathlib.Path(a['executable']).name), '--test-threads=1', *extra]
    if p['name'] in ['mina-p2p', 'mina-node-testing']:
        command = ['unshare', '--user', '--map-root-user', '--net', 'sh', '-c', 'ip link set lo up && exec "$@"',
                   'rpc-test', *command]
    if args.plan:
        print(json.dumps({'suite': name, 'command': command, 'environment': envextra}))
        continue
    print('RUN', name, flush=True)
    start = time.monotonic()
    with (out / (name + '.log')).open('w') as f:
        try:
            code = subprocess.run(command, cwd=cwd, env=dict(env, CARGO_MANIFEST_DIR=str(cwd), **envextra),
                                  stdout=f, stderr=subprocess.STDOUT, timeout=2400).returncode
        except subprocess.TimeoutExpired:
            code = 'timeout'
    result_text = (out / (name + '.log')).read_text(errors='replace')
    summary = re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;', result_text)[-1:]
    failures = [s.split()[1] for s in result_text.splitlines() if s.startswith('test ') and s.endswith('FAILED')]
    r = dict(package=p['name'], suite=name, target=a['target']['name'], exit=code,
             seconds=round(time.monotonic() - start, 2), summary=summary, failed=failures, command=command,
             cwd=str(cwd))
    with (out / 'results.jsonl').open('a') as f:
        f.write(json.dumps(r) + '\n')
    print(json.dumps(r), flush=True)
    if code != 0 or not summary:
        failed.append(name)
assert selected
if failed:
    raise SystemExit('FAILED: ' + ', '.join(failed))
