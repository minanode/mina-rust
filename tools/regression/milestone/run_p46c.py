#!/usr/bin/env python3
"""m9 gate f: a short rerun of P4.6c's isolated private Mesa network on this
build. Adapted from F4's /tmp/f4/run_short.py. The pair runs in a loopback-only
user/network namespace with no route out (no node slots, as the P4.6c
allocation allows), generated throwaway keys under RUN/keys, a private TMPDIR,
full proofs and the Mesa mainnet constants. It stops once OCaml has accepted
TARGET Rust blocks (default 6) and at least 6 minutes have passed, or after
LIMIT seconds (default 1800) of node time, whichever comes first.

Usage: python3 run_p46c.py [TARGET] [LIMIT]
"""
import datetime
import json
import os
import pathlib
import subprocess
import sys
import time

root = pathlib.Path(__file__).resolve().parents[3]
work = pathlib.Path(os.environ['GATE_DIR']).resolve() / 'p46c/run1'
mina = pathlib.Path(os.environ['MINA_BIN']).resolve()
identity = pathlib.Path(os.environ['IDENTITY_BIN']).resolve()
blobs = os.environ['MINA_CIRCUIT_BLOBS_BASE_DIR']
harness = root / 'tools/local-mesa-net'
target = int(sys.argv[1]) if len(sys.argv) > 1 else 6
limit = float(sys.argv[2]) if len(sys.argv) > 2 else 1800
logdir_check = work.parent
logdir_check.mkdir(parents=True, exist_ok=True)
if work.exists():
    raise SystemExit('use a fresh GATE_DIR; private run directory already exists')
children = []
logdir = pathlib.Path(os.environ['GATE_DIR']).resolve() / 'p46c'


def run(args, log, timeout=240):
    with open(log, 'w') as out:
        subprocess.run([str(a) for a in args], cwd=root, stdout=out, stderr=subprocess.STDOUT,
                       check=True, timeout=timeout)


def launch(args, log):
    with open(log, 'w') as out:
        p = subprocess.Popen([str(a) for a in args], cwd=root, stdout=out, stderr=subprocess.STDOUT)
    children.append(p)
    return p


def enter(args):
    return ['bash', harness / 'enter.sh', work, *args]


def stage(s):
    print(datetime.datetime.now(datetime.timezone.utc).isoformat(), s, flush=True)


try:
    stage('prepare private configuration and generated keys')
    run(['python3', harness / 'prepare.py', '--dir', work, '--mina', mina, '--ocaml',
         root / 'scripts/ocaml-mina', '--genesis-delay', '420'], logdir / 'prepare.log')
    stage('start isolated loopback namespace')
    launch(['python3', harness / 'namespace.py', '--dir', work], work / 'logs/namespace.log')
    for _ in range(50):
        if (work / 'namespace.json').exists():
            break
        time.sleep(.2)
    assert (work / 'namespace.json').exists()
    stage('independent genesis identities and full OCaml genesis proof')
    run([identity, '--config', work / 'daemon.json', '--out', work / 'rust-identity.json'],
        work / 'logs/rust-identity.log')
    ocaml = ['env', 'TMPDIR=' + str(work / 'tmp'), str(root / 'scripts/ocaml-mina')]
    run(enter([*ocaml, 'internal', 'chain-id', '--config-file', work / 'daemon.json',
               '--config-directory', work / 'identity-conf', '--genesis-ledger-dir', work / 'genesis']),
        work / 'logs/ocaml-chain-id.log')
    run(enter([*ocaml, 'internal', 'test-genesis-block-generation', '--config-file', work / 'daemon.json',
               '--config-directory', work / 'proof-conf', '--genesis-ledger-dir', work / 'genesis']),
        work / 'logs/ocaml-genesis-proof.log', timeout=360)
    run(['python3', harness / 'match_identity.py', '--dir', work], work / 'logs/identity-match.log')
    start = time.monotonic()
    stage('start OCaml with the explicit isolated allocation')
    launch(['python3', harness / 'run_private.py', 'ocaml', '--dir', work, '--mina', mina, '--blobs', blobs,
            '--private-isolated-allocation'], work / 'logs/ocaml-netns.log')
    for _ in range(90):
        try:
            run(enter(['python3', harness / 'match_identity.py', '--dir', work, '--ocaml-port', '3670']),
                work / 'logs/identity-live.log', timeout=15)
            break
        except subprocess.CalledProcessError:
            time.sleep(2)
    else:
        raise RuntimeError('OCaml GraphQL startup did not complete')
    stage('start Rust producer and sampler')
    launch(['python3', harness / 'run_private.py', 'rust', '--dir', work, '--mina', mina, '--blobs', blobs,
            '--private-isolated-allocation'], work / 'logs/rust-netns.log')
    launch(enter(['python3', harness / 'sample.py', '--dir', work, '--seconds', str(int(limit))]),
           work / 'logs/sample.log')
    while time.monotonic() - start < limit:
        time.sleep(20)
        try:
            run(['python3', harness / 'report.py', '--dir', work], work / 'logs/report.log')
        except subprocess.CalledProcessError:
            continue
        result = json.load(open(work / 'results.json'))
        stage('injected=%s accepted=%s rejections=%s' % (
            result['rust_blocks'], result['ocaml_accepted_rust_blocks'], len(result['ocaml_rejections'])))
        if result['ocaml_accepted_rust_blocks'] >= target and time.monotonic() - start >= 360:
            break
    assert result['ocaml_accepted_rust_blocks'] >= target, 'acceptance target not reached'
    assert not result['ocaml_rejections'], 'OCaml rejected blocks'
    stage('finish measurement and stop only recorded owned processes')
finally:
    if work.exists():
        try:
            run(['python3', harness / 'report.py', '--dir', work], work / 'logs/report-final.log')
        except Exception as error:  # noqa: BLE001
            stage('final report failed: %r' % error)
        run(['python3', harness / 'stop.py', '--dir', work], logdir / 'cleanup.log', timeout=60)
    for child in children:
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.terminate()
            child.wait(timeout=10)
    stage('cleanup complete')
