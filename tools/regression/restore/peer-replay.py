#!/usr/bin/env python3
"""Time an isolated OCaml 4.0.2 best-tip query inside native restore replay.

Requires prepared ocaml-peer.py WORK and private key password in the environment.
The loopback TCP forwarder binds its outgoing source to --isolation-ip, so Rust
can reconnect promptly without admitting every localhost node to OCaml's IP
whitelist. It forwards bytes only; libp2p still authenticates both real peers.
A timing miss is recorded as such, never treated as a passed peer gate.
"""
import argparse
import datetime
import json
import os
import select
import signal
import socket
import socketserver
import subprocess
import threading
import time
from pathlib import Path

import restart as m


def peer_verdict(out, ocaml_work, run_name, result):
    """Keep protocol-level evidence separate from the raw experiment result."""
    def epoch(value):
        return datetime.datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()

    start = epoch(result['replay_begin'].split()[0])
    end = epoch(result['replay_end'].split()[0])
    observations, stale = [], []
    warm_height = result['warm']['synced']['tip']
    for line in (ocaml_work / run_name / 'mina.log').read_text().splitlines():
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        message = entry.get('message', '')
        if not start <= epoch(entry['timestamp']) <= end:
            continue
        if not any(phrase in message for phrase in (
                'best tip', 'best_tip', 'RPC call error', 'proof of the root')):
            continue
        metadata = entry.get('metadata', {})
        item = {'timestamp': entry['timestamp'], 'message': message,
                'metadata': {key: metadata[key] for key in
                             ('length', 'peer', 'error', 'rpc') if key in metadata}}
        observations.append(item)
        if ('Successfully downloaded best tip' in message and
                int(metadata['length']) < warm_height):
            stale.append(item)
    verdict = {'single_peer': result['single_peer'],
               'request_during_replay': result['request_during_replay'],
               'warm_height': warm_height, 'observations': observations,
               'stale_best_tip_during_replay': stale,
               'verdict': 'FAIL: stale best tip served during replay' if stale else
                          'REVIEW: inspect the response/decline evidence'}
    m.save(out / 'peer-verdict.json', verdict)
    return verdict


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('binary', 'lock', 'config', 'out', 'source', 'ocaml-work', 'native-key'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--native-peer', required=True)
    parser.add_argument('--ocaml-peer', required=True)
    parser.add_argument('--run', required=True)
    parser.add_argument('--http', type=int, default=3650)
    parser.add_argument('--p2p', type=int, default=8850)
    parser.add_argument('--ocaml-slot', type=int, default=24)
    parser.add_argument('--proxy-port', type=int, default=8870)
    parser.add_argument('--isolation-ip', default='127.0.0.25')
    parser.add_argument('--restart-delay', type=float, default=12)
    parser.add_argument('--observe', type=float, default=90)
    args = parser.parse_args()
    if not args.isolation_ip.startswith('127.') or args.isolation_ip == '127.0.0.1':
        parser.error('use a separate loopback IP for isolation')
    if (args.http < 3400 or (args.http - 3400) % 10 or
            args.p2p != args.http + 5200 or args.http == 3540):
        parser.error('native ports must be an unprotected agent slot')
    if args.ocaml_slot < 0 or args.ocaml_slot == 14:
        parser.error('choose an unprotected OCaml slot')
    if args.proxy_port < 8600 or (args.proxy_port - 8600) % 10 or args.proxy_port == 8740:
        parser.error('proxy port must be an unprotected agent slot libp2p port')
    for name in ('binary', 'lock', 'config', 'out', 'source', 'ocaml_work', 'native_key'):
        setattr(args, name, getattr(args, name).resolve())
    args.out.mkdir()
    args.network, args.compact, args.chain = 'mainnet', False, m.CHAIN
    args.checkpoint_interval, args.timeout, args.hold = 1800, 900, 0
    ocaml_port = 8600 + 10 * args.ocaml_slot
    args.node_arg = ['--libp2p-keypair', str(args.native_key),
                     '--libp2p-external-ip', args.isolation_ip,
                     '--peers', f'/ip4/127.0.0.1/tcp/{args.proxy_port}/p2p/{args.ocaml_peer}']
    store = args.out / 'data' / m.CHAIN / 'frontier'
    m.copy_store(args.source, store)
    stopping = threading.Event()

    class Forward(socketserver.BaseRequestHandler):
        def handle(self):
            try:
                with socket.socket() as upstream:
                    upstream.settimeout(5)
                    upstream.bind((args.isolation_ip, 0))
                    upstream.connect(('127.0.0.1', ocaml_port))
                    upstream.settimeout(None)
                    streams = [self.request, upstream]
                    while not stopping.is_set():
                        ready, _, _ = select.select(streams, [], [], .5)
                        for source in ready:
                            data = source.recv(65536)
                            if not data:
                                return
                            streams[1 if source is streams[0] else 0].sendall(data)
            except OSError:
                return

    class Server(socketserver.ThreadingTCPServer):
        allow_reuse_address = True
        daemon_threads = True

    server = Server(('127.0.0.1', args.proxy_port), Forward)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    result, run, ocaml, logfile = {}, None, None, None

    def stop(signum, frame):
        raise KeyboardInterrupt(signum)

    signal.signal(signal.SIGTERM, stop)
    try:
        run = m.Run(args, args.out / 'warm', store)
        logfile = (args.out / 'ocaml-supervisor.log').open('w')
        ocaml = subprocess.Popen([
            'python3', str(Path(__file__).with_name('ocaml-peer.py')),
            '--work', str(args.ocaml_work), '--lock', str(args.lock),
            '--peer', f'/ip4/{args.isolation_ip}/tcp/{args.p2p}/p2p/{args.native_peer}',
            '--run', args.run, '--slot', str(args.ocaml_slot), '--duration', '900',
            '--discovery-interval', '5s'], stdout=logfile, stderr=subprocess.STDOUT)
        m.save(args.out / 'supervisors.json', {'driver': os.getpid(), 'ocaml': ocaml.pid})
        run.wait_synced()
        router = None
        while router is None:
            run.poll()
            path = args.ocaml_work / args.run / 'mina.log'
            if path.exists():
                for line in path.read_text().splitlines():
                    try:
                        entry = json.loads(line)
                    except ValueError:
                        continue
                    if entry.get('message') == 'Starting transition router':
                        router = datetime.datetime.strptime(entry['timestamp'], '%Y-%m-%d %H:%M:%S.%fZ').replace(tzinfo=datetime.timezone.utc).timestamp()
                        result['router'] = entry
                        break
            if ocaml.poll() is not None:
                raise RuntimeError('OCaml supervisor exited')
            time.sleep(.05)
        while time.time() < router + args.restart_delay:
            run.poll()
            time.sleep(.02)
        result['stop_utc'] = time.time()
        run.stop()
        result['warm'] = run.summary()
        run = None
        run = m.Run(args, args.out / 'after', store)
        run.wait_synced()
        end = time.monotonic() + args.observe
        while time.monotonic() < end or not run.at_tip:
            run.poll()
            time.sleep(.1)
        lines = run.text.splitlines()
        begin = next(i for i, line in enumerate(lines) if 'TransitionFrontierSyncRestoreInit' in line)
        finish = next(i for i, line in enumerate(lines) if 'kind: "PersistRestored"' in line)
        result['replay_begin'], result['replay_end'] = lines[begin], lines[finish]
        result['queries_during_replay'] = [line for line in lines[begin:finish]
            if 'P2pCatchupRpcQuery' in line and '__Versioned_rpc.Menu' not in line]
        result['all_queries'] = [line for line in lines
            if 'P2pCatchupRpcQuery' in line and '__Versioned_rpc.Menu' not in line]
        peers = set()
        samples = args.ocaml_work / (args.run + '-evidence') / 'status.jsonl'
        for line in samples.read_text().splitlines():
            sample = json.loads(line)
            for peer in ((sample.get('data') or {}).get('daemonStatus') or {}).get('peers', []):
                peers.add(peer['peerId'])
        result['observed_ocaml_peers'] = sorted(peers)
        result['single_peer'] = peers == {args.native_peer}
        result['request_during_replay'] = bool(result['queries_during_replay'])
        run.stop()
        result['after'] = run.summary()
        run = None
        result['checker'] = m.check(args, store, args.out / 'check.log')
        if result['checker']['exit'] or result['after']['panic_logged']:
            raise RuntimeError('native final verification failed')
        if not result['single_peer']:
            raise RuntimeError('OCaml did not remain isolated to the test peer')
        if not result['request_during_replay']:
            raise RuntimeError('no data RPC observed during replay; timing miss')
        verdict = peer_verdict(args.out, args.ocaml_work, args.run, result)
        if verdict['stale_best_tip_during_replay']:
            raise RuntimeError(verdict['verdict'])
    except BaseException as error:
        result['error'] = repr(error)
        raise
    finally:
        if run:
            run.stop()
        if ocaml:
            (args.ocaml_work / (args.run + '-evidence') / 'stop').touch()
            try:
                ocaml.wait(timeout=20)
            except subprocess.TimeoutExpired:
                ocaml.terminate()
                ocaml.wait(timeout=20)
            logfile.close()
        stopping.set()
        server.shutdown()
        server.server_close()
        m.save(args.out / 'result.json', result)
        print(json.dumps(result), flush=True)


if __name__ == '__main__':
    main()
