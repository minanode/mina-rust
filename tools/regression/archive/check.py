#!/usr/bin/env python3
"""Qualify the pinned archive on fresh and upgraded private PostgreSQL schemas.

Requires a Mesa ArchiveRpc payload captured from this node and the compiled
mina-node-common libtest binary. Starts and stops only its own processes.
"""
import argparse
from decimal import Decimal
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
PIN = json.loads((HERE / 'pin.json').read_text())
CASES = ['ff', '00', '610062', '5c78', 'c3a9']


def digest(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def check_file(path, expected):
    actual = digest(path)
    if actual != expected:
        raise RuntimeError(f'SHA-256 mismatch: {path.name}: {actual}')


def free_port(port):
    with socket.socket() as s:
        s.bind(('127.0.0.1', port))


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--archive-prefix', type=Path, required=True)
    p.add_argument('--pg-prefix', type=Path, required=True)
    p.add_argument('--test-binary', type=Path, required=True)
    p.add_argument('--payload', type=Path, required=True)
    p.add_argument('--run-dir', type=Path, required=True, help='must not exist')
    p.add_argument('--pg-port', type=int, default=5440)
    p.add_argument('--archive-port', type=int, default=4669)
    a = p.parse_args()
    for name in ['archive_prefix', 'pg_prefix', 'test_binary', 'payload', 'run_dir']:
        setattr(a, name, getattr(a, name).resolve())
    if a.pg_port == a.archive_port:
        p.error('ports must differ')
    if shutil.disk_usage(a.run_dir.parent).free < 55 * 1024**3:
        p.error('requires at least 55 GiB free')
    for file, sha in PIN['files'].items():
        check_file(a.archive_prefix / file, sha)
    free_port(a.pg_port)
    free_port(a.archive_port)
    a.run_dir.mkdir()  # no existing database or evidence may be overwritten
    (a.run_dir / 'tmp').mkdir()
    env = dict(os.environ, TMPDIR=str(a.run_dir / 'tmp'), MINA_PROFILE='mainnet')
    pg = a.pg_prefix / 'usr/lib/postgresql/16/bin'
    schema = a.archive_prefix / 'etc/mina/archive'
    binary = a.archive_prefix / 'usr/local/bin/mina-archive'
    pg_started = False
    archive = None
    summary = {'pin': PIN, 'payload_sha256': digest(a.payload),
               'test_binary_sha256': digest(a.test_binary), 'paths': {}}

    def run(args, log, **kw):
        with open(a.run_dir / log, 'a') as out:
            return subprocess.run([str(x) for x in args], env=env,
                                  stdout=out, stderr=subprocess.STDOUT,
                                  check=True, timeout=180, **kw)

    def sql(db, query=None, file=None):
        cmd = [str(pg / 'psql'), '-X', '-v', 'ON_ERROR_STOP=1', '-h', '127.0.0.1',
               '-p', str(a.pg_port), '-U', 'archive', '-d', db, '-At']
        cmd += ['-f', str(file)] if file else ['-c', query]
        r = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=120)
        with open(a.run_dir / f'{db}-sql.log', 'a') as out:
            out.write(r.stdout + r.stderr)
        r.check_returncode()
        return r.stdout.strip()

    def rows(db, query):
        return json.loads(sql(db, f"SELECT coalesce(json_agg(t), '[]'::json) FROM ({query}) t"))

    try:
        version = subprocess.check_output([binary, 'version'], env=env, text=True).strip()
        if version != f"Commit {PIN['commit']}":
            raise RuntimeError(f'unexpected archive version: {version}')
        summary['archive_version'] = version
        old = a.run_dir / 'create_schema-4.0.3.sql'
        ref = PIN['previous_schema']['commit']
        url = f'https://raw.githubusercontent.com/MinaProtocol/mina/{ref}/src/app/archive/create_schema.sql'
        with urllib.request.urlopen(url, timeout=30) as response:
            old.write_bytes(response.read())
        check_file(old, PIN['previous_schema']['sha256'])
        run([pg / 'initdb', '-D', a.run_dir / 'pgdata', '-U', 'archive',
             '--auth=trust', '--encoding=UTF8', '--locale=C'], 'initdb.log')
        run([pg / 'pg_ctl', '-D', a.run_dir / 'pgdata', '-l', a.run_dir / 'postgres.log',
             '-o', f"-p {a.pg_port} -h 127.0.0.1 -k ''", '-w', 'start'], 'postgres-control.log')
        pg_started = True
        for mode in ['fresh', 'upgrade']:
            db = f'archive_{mode}'
            uri = f'postgres://archive@127.0.0.1:{a.pg_port}/{db}'
            run([pg / 'createdb', '-h', '127.0.0.1', '-p', str(a.pg_port),
                 '-U', 'archive', db], f'{mode}-create.log')
            sql(db, file=old if mode == 'upgrade' else schema / 'create_schema.sql')
            if mode == 'upgrade':
                values = ','.join(f"(decode('{x}','hex'))" for x in CASES)
                for table in ['token_symbols', 'zkapp_uris']:
                    sql(db, f'INSERT INTO {table}(value) VALUES {values}')
                sql(db, file=schema / 'upgrade.sql')
                for table in ['token_symbols', 'zkapp_uris']:
                    actual = sql(db, f"SELECT encode(value,'hex') FROM {table} ORDER BY 1").splitlines()
                    assert actual == sorted(CASES), (table, actual)
                assert sql(db, "SELECT migration_version || ':' || status FROM migration_history ORDER BY commit_start_at DESC LIMIT 1") == '0.0.3:applied'
                # The same pinned upgrade must be safely repeatable.
                sql(db, file=schema / 'upgrade.sql')
            assert sql(db, "SELECT count(*) FROM information_schema.columns WHERE table_name IN ('token_symbols','zkapp_uris') AND column_name='value' AND data_type='bytea'") == '2'
            assert sql(db, "SELECT count(*) FROM pg_indexes WHERE indexname IN ('idx_user_commands_fee_payer_id','idx_user_commands_source_id','idx_user_commands_receiver_id')") == '3'
            assert sql(db, "SELECT count(*) FROM genesis_accounts") == '0'

            # Untouched accounts come from each era's actual runtime config,
            # not from live block effects. Exercise the explicit OCaml tool.
            accounts = json.loads((ROOT / 'crates/node/src/daemon_json/testdata/ocaml-ledger-export-sample.json').read_text())
            config = {'ledger': {'accounts': accounts, 'add_genesis_winner': False}}
            toolbox = a.archive_prefix / 'usr/local/bin/mina-archive-hardfork-toolbox'
            for genesis_height in [1, 101]:
                if genesis_height == 101:
                    config['proof'] = {'fork': {
                        'blockchain_length': 100, 'global_slot_since_genesis': 100,
                        'state_hash': '3NLvdGJk7DjwHeMW9Hhmv2AMrRnaou5NXfU2KMfBF3MXx6TEQEui'}}
                config_path = a.run_dir / f'{mode}-genesis-{genesis_height}.json'
                config_path.write_text(json.dumps(config))
                run([toolbox, 'populate-genesis-accounts', '--config-file', config_path,
                     '--postgres-uri', uri], f'{mode}-genesis-{genesis_height}.log')
                actual = rows(db, f'SELECT public_key, balance, nonce FROM genesis_accounts WHERE genesis_height={genesis_height} ORDER BY public_key')
                wanted = sorted([{'public_key': acc['pk'],
                    'balance': str(int(Decimal(acc['balance']) * 10**9)),
                    'nonce': int(acc.get('nonce', 0))} for acc in accounts], key=lambda x: x['public_key'])
                assert actual == wanted, (genesis_height, actual)
            assert sql(db, 'SELECT count(*) FROM genesis_accounts') == str(2 * len(accounts))
            assert sql(db, 'SELECT count(*) FROM accounts_accessed') == '0'
            archive_log = a.run_dir / f'{mode}-archive.log'
            with open(archive_log, 'w') as out:
                archive = subprocess.Popen([binary, 'run', '--postgres-uri', uri,
                    '--server-port', str(a.archive_port), '--log-json'], env=env,
                    stdout=out, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 30
            while True:
                if archive.poll() is not None:
                    raise RuntimeError(f'archive exited: {archive_log.name}')
                if 'Archive process ready. Clients can now connect' in archive_log.read_text():
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError('archive did not become ready')
                time.sleep(.2)
            results = []
            for case in [None] + CASES:
                label = case or 'original'
                sql(db, 'TRUNCATE blocks CASCADE')
                expected_file = a.run_dir / f'{mode}-{label}-expected.json'
                test_env = dict(env, ARCHIVE_404_PAYLOAD=str(a.payload),
                    ARCHIVE_404_EXPECTED=str(expected_file),
                    ARCHIVE_404_ADDRESS=f'127.0.0.1:{a.archive_port}')
                test_env.pop('ARCHIVE_404_BYTES', None)
                if case:
                    test_env['ARCHIVE_404_BYTES'] = case
                r = subprocess.run([a.test_binary, 'ocaml_404_archive_bytes', '--ignored', '--nocapture'],
                    env=test_env, capture_output=True, text=True, timeout=90)
                (a.run_dir / f'{mode}-{label}-delivery.log').write_text(r.stdout + r.stderr)
                r.check_returncode()
                if '1 passed' not in r.stdout:
                    raise RuntimeError('required Rust regression did not run')
                expected = json.loads(expected_file.read_text())
                deadline = time.monotonic() + 30
                while True:
                    actual = rows(db, '''SELECT b.height, b.state_hash, b.parent_hash,
                        (SELECT json_agg(x ORDER BY x.index) FROM
                          (SELECT aa.ledger_index AS index, encode(ts.value,'hex') AS symbol,
                                  encode(u.value,'hex') AS uri
                           FROM accounts_accessed aa JOIN token_symbols ts ON ts.id=aa.token_symbol_id
                           LEFT JOIN zkapp_accounts za ON za.id=aa.zkapp_id
                           LEFT JOIN zkapp_uris u ON u.id=za.zkapp_uri_id
                           WHERE aa.block_id=b.id) x) AS accounts FROM blocks b''')
                    expected['accounts'].sort(key=lambda x: x['index'])
                    if actual == [expected]:
                        break
                    if time.monotonic() > deadline:
                        raise RuntimeError(f'{mode}/{label}: committed rows differ: {actual}')
                    time.sleep(.2)
                results.append({'case': label, 'blocks': len(actual),
                                'accounts': len(expected['accounts']), 'exact_match': True})
            stop(archive)
            archive = None
            errors = []
            for line in archive_log.read_text().splitlines():
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if str(entry.get('level', '')).lower() in ['error', 'fatal']:
                    errors.append(entry)
            assert not errors, errors
            summary['paths'][mode] = {'cases': results, 'archive_errors': 0,
                                     'genesis_accounts': 2 * len(accounts), 'genesis_heights': [1, 101],
                                     'schema': 'bytea, genesis_accounts, three account indexes'}
        # Rollback is only representable for text-compatible values. Check the
        # script's actual failure, not merely a stand-alone conversion function.
        summary['downgrade'] = {}
        for value in ['c3a9', 'ff', '00']:
            db = f'rollback_{value}'
            run([pg / 'createdb', '-h', '127.0.0.1', '-p', str(a.pg_port),
                 '-U', 'archive', db], f'{db}-create.log')
            sql(db, file=schema / 'create_schema.sql')
            sql(db, file=schema / 'upgrade.sql')
            for table in ['token_symbols', 'zkapp_uris']:
                sql(db, f"INSERT INTO {table}(value) VALUES (decode('{value}','hex'))")
            try:
                sql(db, file=schema / 'downgrade.sql')
            except subprocess.CalledProcessError as error:
                assert value != 'c3a9', error.stderr
                assert 'invalid byte sequence' in error.stderr, error.stderr
                for table in ['token_symbols', 'zkapp_uris']:
                    assert sql(db, f"SELECT encode(value,'hex') FROM {table}") == value
                summary['downgrade'][value] = 'rejected unrepresentable text; bytea value retained'
            else:
                assert value == 'c3a9', 'unrepresentable value unexpectedly downgraded'
                for table in ['token_symbols', 'zkapp_uris']:
                    assert sql(db, f"SELECT encode(convert_to(value,'UTF8'),'hex') FROM {table}") == value
                summary['downgrade'][value] = 'UTF-8 bytes preserved as text'
        summary['passed'] = True
    finally:
        stop(archive)
        if pg_started:
            run([pg / 'pg_ctl', '-D', a.run_dir / 'pgdata', '-m', 'fast', '-w', 'stop'],
                'postgres-control.log')
        (a.run_dir / 'result.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
