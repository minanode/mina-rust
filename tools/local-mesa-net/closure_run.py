#!/usr/bin/env python3
"""Bounded isolated production with payments, catch-up and ledger closure."""
import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

from processes import snapshot, track


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dir', type=Path, required=True)
    parser.add_argument('--mina', type=Path, required=True)
    parser.add_argument('--identity', type=Path, required=True)
    parser.add_argument('--blobs', type=Path, required=True)
    parser.add_argument('--absolute-stop-unix', type=float)
    parser.add_argument('--reuse-tmp', type=Path)
    parser.add_argument('--external-workers', type=int, default=6)
    parser.add_argument('--minimum-seconds', type=int, default=0)
    parser.add_argument('--ledger-advances', type=int, default=1)
    parser.add_argument('--max-load', type=float, default=32)
    parser.add_argument('--delay-own-block', action='store_true',
                        help="pause this run's Rust PID for 105 seconds while a proof is pending")
    parser.add_argument('--delay-scheduled-slot', action='store_true',
                        help='pause this run producer across an entire unstarted won slot')
    args = parser.parse_args()
    assert (args.blobs/'4.0.2devnet').is_dir(), '--blobs must name the parent circuit-blobs directory'
    assert args.mina.is_file() and args.identity.is_file()
    work = args.dir.resolve()
    root = Path(__file__).resolve().parents[2]
    harness = root/'tools/local-mesa-net'
    os.umask(0o077)
    children = []

    def command(argv, log, env=None, timeout=180):
        with log.open('w') as output:
            subprocess.run([str(x) for x in argv], cwd=root, env=env,
                           stdout=output, stderr=subprocess.STDOUT, timeout=timeout, check=True)

    def enter(argv):
        return ['bash', harness/'enter.sh', work, *argv]

    def spawn(argv, log):
        with log.open('w') as output:
            child = subprocess.Popen([str(x) for x in argv], cwd=root,
                                     stdout=output, stderr=subprocess.STDOUT)
        children.append(child)
        return child

    def status(message):
        print(datetime.datetime.now(datetime.timezone.utc).isoformat(), message, flush=True)

    def wait_for_capacity(stage):
        for _ in range(240):
            if max(os.getloadavg()) <= args.max_load:
                return
            status(f'{stage}: waiting for host load <= {args.max_load}: {os.getloadavg()}')
            time.sleep(30)
        raise RuntimeError(f'{stage}: host load remained above limit for two hours')

    # Wait before creating time-dependent genesis or starting any node.
    wait_for_capacity('before genesis setup')
    try:
        command(['python3', harness/'prepare.py', '--dir', work, '--mina', args.mina,
                 '--ocaml', root/'scripts/ocaml-mina', '--network', 'devnet',
                 '--genesis-delay', '300'], work.parent/'logs/prepare.log')
        if args.reuse_tmp:
            assert args.reuse_tmp.is_dir()
            (work/'tmp').rmdir()
            (work/'tmp').symlink_to(args.reuse_tmp.resolve(), target_is_directory=True)
        ns = spawn(['python3', harness/'namespace.py', '--dir', work], work/'logs/namespace.log')
        for _ in range(100):
            if (work/'namespace.json').exists():
                break
            assert ns.poll() is None, 'namespace keeper failed'
            time.sleep(.1)
        assert (work/'namespace.json').exists()
        command(enter(['python3', '-c', "import json,subprocess; print(json.dumps([{'family':f,'returncode':p.returncode,'stderr':p.stderr} for f,cmd in [('IPv4',['ip','route','get','192.0.2.1']),('IPv6',['ip','-6','route','get','2001:db8::1'])] for p in [subprocess.run(cmd,capture_output=True,text=True)]],indent=2))"]), work/'unreachable-routes.json')
        status('isolated namespace ready; computing identities and full genesis proof')
        command([args.identity, '--network', 'devnet', '--config', work/'daemon.json',
                 '--out', work/'rust-identity.json'], work/'logs/rust-identity.log')
        env = {k:v for k,v in os.environ.items() if not k.startswith(('MINA_', 'OCAML_'))}
        env.update(TMPDIR=str(work/'tmp'), MINA_PROFILE='devnet',
                   OMP_NUM_THREADS='8', RAYON_NUM_THREADS='8')
        for label, internal, conf in [('chain-id', 'chain-id', 'identity-conf'),
                                     ('genesis-proof', 'test-genesis-block-generation', 'proof-conf')]:
            command(enter([root/'scripts/ocaml-mina', 'internal', internal,
                           '--config-file', work/'daemon.json', '--config-directory', work/conf,
                           '--genesis-ledger-dir', work/'genesis']),
                    work/f'logs/ocaml-{label}.log', env=env, timeout=300)
        command(['python3', harness/'match_identity.py', '--dir', work], work/'logs/identity-match.log')
        if args.absolute_stop_unix:
            now = time.time()
            assert 7200 < args.absolute_stop_unix-now <= 14400
            (work/'node-deadline.json').write_text(json.dumps({
                'started_unix':now, 'stop_unix':args.absolute_stop_unix,
                'max_seconds':args.absolute_stop_unix-now},indent=2)+'\n')
        wait_for_capacity('after genesis setup')
        shared = ['--dir', work, '--mina', args.mina, '--blobs', args.blobs,
                  '--private-isolated-allocation', '--max-seconds', '14400']
        ocaml = spawn(['python3', harness/'run_private.py', 'ocaml', *shared,
                       '--ocaml-snark-worker'], work/'logs/ocaml-netns.log')
        for _ in range(120):
            assert ocaml.poll() is None, 'OCaml exited during startup'
            try:
                command(enter(['python3', harness/'match_identity.py', '--dir', work,
                               '--ocaml-port', '3670']), work/'logs/live-identity-match.log', timeout=15)
                break
            except subprocess.CalledProcessError:
                time.sleep(1)
        else:
            raise RuntimeError('OCaml live identity not ready')
        rust = spawn(['python3', harness/'run_private.py', 'rust', *shared], work/'logs/rust-netns.log')
        started = time.time()
        (work/'production-window.json').write_text(json.dumps({'started_unix': started,
            'minimum_seconds': args.minimum_seconds, 'stop_unix': json.loads((work/'node-deadline.json').read_text())['stop_unix'],
            'worker': f'OCaml 4.0.2: one daemon worker (2 requested threads), {args.external_workers} external workers (OMP/Rayon 2 each)',
            'worker_fee_nanomina': 10000000, 'pending_payment_target': 128,
            'payment_amount_nanomina': 1000000, 'payment_fee_nanomina': 100000000}, indent=2)+'\n')
        for i in range(args.external_workers):
            child = spawn(enter(['nice', '-n', '10', 'env', f'TMPDIR={work / "tmp"}', 'MINA_PROFILE=devnet', 'OMP_NUM_THREADS=2', 'RAYON_NUM_THREADS=2',
                                 root/'scripts/ocaml-mina', 'internal', 'snark-worker',
                                 '--daemon-address', '127.0.0.1:8871', '--proof-level', 'full',
                                 '--config-directory', work/f'worker-{i}', '--log-json']), work/f'logs/worker-{i}.log')
            (work/f'worker-{i}.pid').write_text(str(child.pid)+'\n')
        sampler = spawn(enter(['python3', harness/'closure_sample.py', '--dir', work]), work/'logs/sampler.log')
        shutil.copyfile(harness/'load_guard.py', work/'load-guard.py')
        load_guard = spawn(['python3', harness/'load_guard.py', '--dir', work,
                            '--max-load', str(args.max_load)], work/'logs/load-guard.log')
        status(f'Rust producer, OCaml observer and {args.external_workers+1} OCaml workers started')
        next_nonce = 0
        stop = json.loads((work/'node-deadline.json').read_text())['stop_unix'] - 30
        last_tip = None
        first_accepted = None
        delayed = False
        scheduled_delayed = False

        def pause_producer(attempt, duration, filename, method):
            pid = int((work/'rust.pid').read_text())
            namespace = json.loads((work/'namespace.json').read_text())['netns']
            assert os.readlink(f'/proc/{pid}/ns/net') == namespace
            owned = track(work)
            assert str(pid) in owned
            record = {'pid':pid, 'start':owned[str(pid)]['start'],
                      'began_unix':time.time(), 'duration_seconds':duration,
                      'attempt':attempt, 'method':method}
            marker = work/'manual-producer-pause.json'
            marker.write_text(json.dumps(record)+'\n')
            os.kill(pid, signal.SIGSTOP)
            try:
                time.sleep(duration)
                # A deliberate pause must not override a host-capacity wait.
                while time.time() < stop-60:
                    assert load_guard.poll() is None, 'load guard exited during test pause'
                    guard = work/'load-guard-state.json'
                    paused = guard.exists() and json.loads(guard.read_text())['paused']
                    if not paused and max(os.getloadavg()) <= args.max_load:
                        break
                    time.sleep(10)
            finally:
                assert snapshot().get(pid, {}).get('start') == record['start']
                marker.unlink()
                os.kill(pid, signal.SIGCONT)
                record['resumed_unix'] = time.time()
                (work/filename).write_text(json.dumps(record, indent=2)+'\n')

        while time.time() < stop:
            track(work)
            assert rust.poll() is None, 'Rust exited during run'
            assert ocaml.poll() is None, 'OCaml exited during run'
            assert sampler.poll() is None, 'sampler exited during run'
            assert load_guard.poll() is None, 'load guard exited during run'
            if shutil.disk_usage(work).free < 55_000_000_000:
                # Delete only reconstructible outputs owned by this run; preserve evidence.
                for directory in (work/'blocks', work/'tmp'):
                    if directory.exists() and directory.name == 'blocks':
                        shutil.rmtree(directory)
                        directory.mkdir()
                if shutil.disk_usage(work).free < 55_000_000_000:
                    raise RuntimeError('disk headroom below 55 GB after own archive cleanup')
            time.sleep(10)
            path = work/'live-summary.json'
            if not path.exists():
                continue
            summary = json.loads(path.read_text())
            if first_accepted is None and summary['accepted_blocks_including_genesis'] >= 2:
                first_accepted = time.time()
                path_window = work/'production-window.json'
                window = json.loads(path_window.read_text())
                window['first_accepted_unix'] = first_accepted
                path_window.write_text(json.dumps(window,indent=2)+'\n')
            if summary['tip'] != last_tip:
                status(f"accepted={summary['accepted_blocks_including_genesis']-1} tx={summary['transactions']} works={summary['works_bought']} advances={summary['snarked_ledger_advances']}")
                last_tip = summary['tip']
            if first_accepted and time.time()-first_accepted >= args.minimum_seconds and summary['snarked_ledger_advances'] >= args.ledger_advances:
                status('requested duration and ledger advances reached')
                break
            attempt = summary['rust_status'].get('current_block_production_attempt') or {}
            if (args.delay_own_block and not delayed
                    and summary['accepted_blocks_including_genesis'] >= 4
                    and attempt.get('status') == 'ProofCreatePending'):
                command(enter(['python3', '-c',
                    "import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:3660/status',timeout=10).read().decode())"]),
                    work/'delay-status.json', timeout=15)
                attempt = json.loads((work/'delay-status.json').read_text()).get('current_block_production_attempt') or {}
                if attempt.get('status') != 'ProofCreatePending':
                    continue
                status('delaying own pending block proof by pausing its daemon for 105 seconds')
                pause_producer(attempt, 105, 'delayed-proof.json',
                               'SIGSTOP/SIGCONT of own Rust daemon during proof')
                delayed = True
                status('own producer resumed; descendants must recover without replay')
            if (args.delay_scheduled_slot and not scheduled_delayed
                    and summary['accepted_blocks_including_genesis'] >= 9
                    and attempt.get('status') == 'Scheduled'
                    and 0 < attempt['won_slot']['slot_time']/1e9-time.time() <= 25):
                command(enter(['python3', '-c',
                    "import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:3660/status',timeout=10).read().decode())"]),
                    work/'schedule-pause-status.json', timeout=15)
                attempt = json.loads((work/'schedule-pause-status.json').read_text()).get('current_block_production_attempt') or {}
                if attempt.get('status') == 'Scheduled':
                    duration = attempt['won_slot']['slot_time']/1e9 + 95 - time.time()
                    if 95 <= duration <= 120:
                        status('pausing own producer across an unstarted won slot')
                        pause_producer(attempt, duration, 'schedule-pause.json',
                                       'SIGSTOP/SIGCONT across a complete unstarted won slot')
                        scheduled_delayed = True
                        status('producer resumed; expired slot must be discarded and production continue')
            nonce = summary.get('sender_nonce')
            if nonce is not None and summary['accepted_blocks_including_genesis'] >= 2 and not any(summary['graphql_errors'].values()):
                count = 128 - (next_nonce-int(nonce))
                if count >= 32:
                    count = min(128, count)
                    label = f'payments-{next_nonce:06d}'
                    command(enter(['python3', harness/'payment.py', '--dir', work, '--mina', args.mina,
                                   '--nonce', str(next_nonce), '--count', str(count), '--amount', '1000000',
                                   '--label', label]), work/f'logs/{label}.log', timeout=240)
                    reply = json.loads((work/f'{label}-response.json').read_text())
                    assert isinstance(reply, list) and len(reply) == count, f'payment batch rejected: {label}'
                    next_nonce += count
                    status(f'payment batch accepted, next nonce {next_nonce}')
        command(enter(['python3', harness/'audit.py', '--dir', work, '--label', 'final']), work/'logs/audit-final.log')
        command(enter(['python3', '-c', "import json,os,subprocess; print(json.dumps({'netns':os.readlink('/proc/self/ns/net'),'interfaces':json.loads(subprocess.check_output(['ip','-json','address'])),'routes':json.loads(subprocess.check_output(['ip','-json','route','show','table','all'])),'routes6':json.loads(subprocess.check_output(['ip','-6','-json','route','show','table','all']))},indent=2))"]), work/'isolation-during-production.json')
        status('final audit and isolation captured')
    finally:
        if work.exists():
            if (work/'namespace.json').exists():
                nsname = json.loads((work/'namespace.json').read_text())['netns']
                track(work, namespace=nsname)
            command(['python3', harness/'stop.py', '--dir', work], work/'logs/cleanup.log', timeout=40)
            for child in children:
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            if (work/'keys').exists():
                shutil.rmtree(work/'keys')
            (work/'logs/key-generation.log').unlink(missing_ok=True)
            if not args.reuse_tmp and (work/'tmp').is_dir():
                shutil.rmtree(work/'tmp')
            current = snapshot()
            known = json.loads((work/'processes.json').read_text()) if (work/'processes.json').exists() else {}
            namespace_members = []
            for pid in current:
                try:
                    if (work/'namespace.json').exists() and os.readlink(f'/proc/{pid}/ns/net') == nsname:
                        namespace_members.append(pid)
                except OSError:
                    pass
            cleanup = {'keys_deleted': not (work/'keys').exists(),
                       'namespace_members': namespace_members,
                       'tracked_processes': [pid for pid, row in known.items()
                                             if current.get(int(pid), {}).get('start') == row['start']]}
            (work/'cleanup-verification.json').write_text(json.dumps(cleanup, indent=2)+'\n')
            assert cleanup['keys_deleted'] and not namespace_members and not cleanup['tracked_processes'], cleanup
            status('owned processes stopped and throwaway keys deleted')


if __name__ == '__main__':
    main()
