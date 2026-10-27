#!/usr/bin/env python3
"""Join raw sustained-run observations without treating receipt as acceptance."""
import argparse
from collections import Counter
import csv
import datetime
import gzip
import json
from pathlib import Path
import re


def height(block):
    return int(block['protocolState']['consensusState']['blockHeight'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dir', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    work, out = args.dir, args.out
    out.mkdir(parents=True, exist_ok=True)
    blocks = {'rust': {}, 'ocaml': {}}
    first = {'rust': {}, 'ocaml': {}}
    attempts = {}
    parity = {}
    account_observations = {'rust': {}, 'ocaml': {}}
    differences = []
    tip_episodes = []
    tip_episode = None
    errors = Counter()
    sample_count = 0
    final = None
    last_sample = None
    incomplete_tail_samples = 0
    scans = {}
    sample_path=work/'samples.jsonl'
    sample_stream=sample_path.open() if sample_path.exists() else gzip.open(work/'samples.jsonl.gz','rt')
    with sample_stream as lines:
        for line in lines:
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            last_sample = row
            # A host-capacity pause can leave a tail of RPC timeouts. Keep
            # those errors and their timing, but anchor chain/account checks
            # to the latest complete observation from both independent nodes.
            # The verifier still requires coverage of every injected block.
            if all((row[side].get('data') or {}).get('bestChain')
                   and not row[side].get('errors') and not row[side].get('sample_error')
                   for side in blocks) and row.get('rust_status', {}).get('transition_frontier'):
                final = row
                incomplete_tail_samples = 0
            else:
                incomplete_tail_samples += 1
            sample_count += 1
            tips = {}
            for side in blocks:
                response = row[side]
                if response.get('errors') or response.get('sample_error'):
                    errors[(side, json.dumps(response.get('errors', response.get('sample_error')), sort_keys=True))] += 1
                chain = (response.get('data') or {}).get('bestChain') or []
                for block in chain:
                    key = block['stateHash']
                    blocks[side][key] = block
                    first[side].setdefault(key, row['time'])
                if chain:
                    tips[side] = max(chain, key=height)['stateHash']
            for attempt in (row.get('producer') or {}).get('attempts', []):
                key = str(attempt['won_slot']['global_slot'])
                attempts[key] = attempt
            if set(tips) == {'rust', 'ocaml'}:
                account = {side: {k:v for k,v in row[side]['data'].items() if k!='bestChain'} for side in blocks}
                for side in blocks:
                    account_observations[side][tips[side]] = {'time':row['time'], 'accounts':account[side]}
                if tips['rust'] == tips['ocaml']:
                    if tip_episode:
                        tip_episode['resolved_at'] = row['time']
                        tip_episodes.append(tip_episode)
                        tip_episode = None
                    key = tips['rust']
                    snap = {'time': row['time'], 'hash': key, **account}
                    if account['rust'] == account['ocaml']:
                        parity[key] = snap
                    else:
                        differences.append({'kind': 'accounts_at_equal_tip', **snap})
                else:
                    if tip_episode is None:
                        tip_episode = {'start':row['time'], 'first_tips':tips, 'samples':0, 'resolved_at':None}
                    tip_episode.update(last=row['time'], last_tips=tips)
                    tip_episode['samples'] += 1
                    differences.append({'kind': 'tips', 'time': row['time'], 'tips': tips})
            if row.get('scan_summary'):
                scans[row['scan_summary']['hash']] = row['scan_summary']
    verified = {}
    injected = {}
    rust_errors = []
    rust_warnings = []
    rpc_queries = []
    slot_discards = []
    for line in (work/'logs/rust-netns.log').open():
        match = re.search(r'block_hash: "([^"]+)"', line)
        if 'Block proof verified in prover service' in line and match:
            verified.setdefault(match.group(1), line.strip())
        if 'kind: "BlockProducerBlockInjected"' in line and match:
            injected.setdefault(match.group(1), line.strip())
        if 'kind: "P2pCatchupRpcQuery"' in line:
            rpc_queries.append(line.strip())
        if 'kind: "BlockProducerWonSlotDiscard"' in line:
            slot_discards.append(line.strip())
        if re.search(r'\s(?:WARN|ERROR)\s', line):
            rust_warnings.append(line.strip())
        if (any(token in line for token in ('panicked at', 'ProveError', 'proof verification failed', 'BlockProducerBlockDiscarded', 'BlockProducerWonSlotDiscard'))
                and 'reason: "WonSlotExpired"' not in line):
            rust_errors.append(line.strip())
    events = []
    frontier = {}
    received = {}
    # OCaml rotates its JSON log. Preserve every selected event once.
    seen = set()
    for path in sorted((work/'ocaml-netns').glob('mina.log*')):
        opener = gzip.open if path.suffix == '.gz' else open
        # OCaml JSON can contain byte strings (for example SOK digests),
        # including non-UTF-8 bytes. Preserve them one-to-one, as for its
        # archived block JSON; event names, hashes and timestamps are ASCII.
        with opener(path, 'rt', encoding='latin-1') as lines:
            for line in lines:
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                message = event.get('message', '')
                selected = (message in ('Saw block with state hash $state_hash', 'Received a block from $sender')
                            or 'catchup' in message.lower()
                            or message.startswith('Downloader')
                            or 'was rejected for reason' in message
                            or message == 'RPC call error for $rpc'
                            or event.get('level') in ('Error', 'Fatal', 'Faulty_peer')
                            or ('snark' in message.lower() and any(x in message.lower() for x in ('completed', 'submitted', 'failed', 'error'))))
                if selected and line not in seen:
                    seen.add(line)
                    events.append(event)
                if message == 'Saw block with state hash $state_hash':
                    frontier.setdefault(event['metadata']['state_hash'], event['timestamp'])
                if message == 'Received a block from $sender':
                    received.setdefault(event['metadata']['state_hash'], event['timestamp'])
    events.sort(key=lambda e:e['timestamp'])
    chains = {}
    for side in blocks:
        latest = ((final[side].get('data') or {}).get('bestChain') or []) if final else []
        chains[side] = []
        if latest:
            block = max(latest, key=height)
            while True:
                chains[side].append(block['stateHash'])
                parent = block['protocolState']['previousStateHash']
                if parent not in blocks[side]:
                    break
                block = blocks[side][parent]
            chains[side].reverse()
    chain = chains['ocaml']
    rows = []
    for key in sorted(set(blocks['rust']) | set(blocks['ocaml']), key=lambda k:height(blocks['ocaml'].get(k) or blocks['rust'][k])):
        b = blocks['ocaml'].get(key) or blocks['rust'][key]
        attempt = next((v for v in attempts.values() if (v.get('block') or {}).get('hash') == key), None)
        rows.append({'hash': key, 'height': height(b), 'canonical': key in chain,
                     'rust': blocks['rust'].get(key), 'ocaml': blocks['ocaml'].get(key),
                     'first_observed': {side:first[side].get(key) for side in blocks},
                     'attempt': attempt, 'service_self_verified_log': verified.get(key),
                     'injected_log': injected.get(key), 'ocaml_frontier_log': frontier.get(key),
                     'ocaml_received_log': received.get(key),
                     'scan_summary': scans.get(key), 'account_parity': parity.get(key),
                     'account_observations': {side:account_observations[side].get(key) for side in blocks}})
    if tip_episode:
        tip_episodes.append(tip_episode)
    result = {'sample_count': sample_count, 'canonical_chain': chain, 'blocks': rows,
              'all_injected_logs': injected, 'all_self_verified_logs': verified,
              'rust_canonical_chain': chains['rust'],
              'attempts': attempts, 'observation_differences': differences,
              'tip_difference_episodes':tip_episodes,
              'sample_errors': [{'side':s, 'error':json.loads(e), 'count':n} for (s,e),n in errors.items()],
              'rpc_query_logs': rpc_queries,
              'slot_discard_logs': slot_discards,
              'rust_error_events': rust_errors, 'rust_warning_error_events':rust_warnings,
              'final_observation': final,
              'last_sample_time': last_sample['time'] if last_sample else None,
              'incomplete_tail_samples': incomplete_tail_samples}
    with gzip.open(out/'observations.json.gz', 'wt') as f:
        json.dump(result, f, indent=2)
    with gzip.open(out/'ocaml-events.json.gz', 'wt') as f:
        json.dump(events, f, indent=2)
    scan_progress = []
    for path in sorted((work/'scan').glob('*.json.gz'), key=lambda p:p.stat().st_mtime):
        with gzip.open(path, 'rt') as f:
            scan = json.load(f)
        scan_progress.append({'hash':path.name.removesuffix('.json.gz'),
            'trees_newest_first': [dict(Counter(j['status'] for j in tree))
                                   for tree in scan['scan_state']]})
    (out/'scan-progress.json').write_text(json.dumps(scan_progress, indent=2)+'\n')
    # Public evidence allowlist. Never traverse keys, private cache or witnesses.
    for name in ('daemon.json','network.json','identity-match.json','rust-identity.json',
                 'ocaml-genesis.json','ocaml-initial-graphql.json',
                 'namespace.json','unreachable-routes.json','rust-isolation-preflight.json',
                 'ocaml-isolation-preflight.json','isolation-during-production.json',
                 'production-window.json','node-deadline.json','audit-final.json',
                 'cleanup-verification.json','observation-query.graphql','fee-correction.json',
                 'snark-pool-first.json','additional-workers.json','worker-priority-adjustment.json',
                 'delayed-proof.json','build.json','host-resources.jsonl',
                 'schedule-pause.json','load-guard-state.json',
                 'load-guard.jsonl','load-guard.py','load-guard-initial.py','load-guard-eight.py',
                 'load-guard-six.py','full-proof-pause.json',
                 'scheduler-stall.json','termination.json',
                 'own-output-cleanup.json',
                 'healthy-empty.json','healthy-empty-sample.json.gz','healthy-empty-scan.json.gz'):
        path = work/name
        if path.exists():
            (out/name).write_bytes(path.read_bytes())
    canonical = [blocks['ocaml'][key] for key in chain]
    roots = [b['protocolState']['blockchainState']['snarkedLedgerHash'] for b in canonical]
    summary = {'observed_rust_blocks': len(injected),
               'accepted_rust_blocks': sum(k in blocks['ocaml'] for k in injected),
               'canonical_produced_blocks': sum(k in injected for k in chain),
               'snarked_ledger_advances': sum(a!=b for a,b in zip(roots,roots[1:])),
               'works_bought': sum(len(b['snarkJobs']) for b in canonical),
               'payments': sum(len(b['transactions']['userCommands']) for b in canonical),
               'sample_count': sample_count, 'account_parity_tip_count': len(parity),
               'observation_difference_count': len(differences),
               'ocaml_rejections': sum('was rejected for reason' in e.get('message','') for e in events)}
    rust_canonical = [blocks['rust'][key] for key in chains['rust']]
    rust_roots = [b['protocolState']['blockchainState']['snarkedLedgerHash'] for b in rust_canonical]
    summary.update(rust_works_bought=sum(len(b['snarkJobs']) for b in rust_canonical),
                   rust_payments=sum(len(b['transactions']['userCommands']) for b in rust_canonical),
                   rust_snarked_ledger_advances=sum(a!=b for a,b in zip(rust_roots,rust_roots[1:])),
                   final_tip_agreement=chains['rust'][-1:]==chains['ocaml'][-1:],
                   catchup_rpc_errors=sum(e.get('message')=='RPC call error for $rpc' and e.get('metadata',{}).get('rpc')=='get_transition_chain_proof' for e in events))
    (out/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    with (out/'blocks.csv').open('w', newline='') as f:
        fields = ['height','hash','producer','producer_public_key','private_slot',
                  'slot_since_genesis','payments','failed_payments','works_bought',
                  'payment_fees_nanomina','work_fees_nanomina','coinbase_nanomina',
                  'snarked_ledger_hash','staged_ledger_hash','ledger_proof_emitted',
                  'accepted_by_ocaml','ocaml_frontier_time','canonical',
                  'proof_seconds','injected_at','gossip_deadline','injected_before_gossip_deadline',
                  'received_by_ocaml','observation_source']
        writer = csv.DictWriter(f, fieldnames=fields, lineterminator='\n')
        writer.writeheader()
        for row in rows:
            if not row['injected_log']:
                continue
            b = row['ocaml'] or row['rust']
            state = b['protocolState']['blockchainState']
            txs = b['transactions']['userCommands']
            attempt = row['attempt'] or {}
            times = attempt.get('times') or {}
            proof_seconds = ((times['proof_create_end']-times['proof_create_start'])/1e9
                             if times.get('proof_create_end') and times.get('proof_create_start') else None)
            config = json.loads((work/'daemon.json').read_text())
            private_slot = attempt.get('won_slot',{}).get('global_slot')
            injected_at = row['injected_log'].split(' ',1)[0]
            start = datetime.datetime.fromisoformat(config['genesis']['genesis_state_timestamp'].replace('Z','+00:00')).timestamp()
            deadline = start + (private_slot+config['genesis']['delta']+1)*config['proof']['block_window_duration_ms']/1000
            writer.writerow({'height':row['height'],'hash':row['hash'],'producer':'Rust',
                'producer_public_key':b['creator'],
                'private_slot':private_slot,
                'slot_since_genesis':b['protocolState']['consensusState']['slotSinceGenesis'],
                'payments':len(txs),'failed_payments':sum(t['failureReason'] is not None for t in txs),
                'works_bought':len(b['snarkJobs']),
                'payment_fees_nanomina':sum(int(t['fee']) for t in txs),
                'work_fees_nanomina':sum(int(w['fee']) for w in b['snarkJobs']),
                'coinbase_nanomina':b['transactions']['coinbase'],
                'snarked_ledger_hash':state['snarkedLedgerHash'],
                'staged_ledger_hash':state['stagedLedgerHash'],
                'ledger_proof_emitted':state['stagedLedgerProofEmitted'],
                'accepted_by_ocaml':row['ocaml'] is not None,
                'ocaml_frontier_time':row['ocaml_frontier_log'],'canonical':row['canonical'],
                'proof_seconds':round(proof_seconds,3) if proof_seconds is not None else None,
                'injected_at':injected_at,
                'gossip_deadline':datetime.datetime.fromtimestamp(deadline,datetime.timezone.utc).isoformat(),
                'injected_before_gossip_deadline':datetime.datetime.fromisoformat(injected_at.replace('Z','+00:00')).timestamp()<deadline,
                'received_by_ocaml':bool(row['ocaml_received_log']),
                'observation_source':'OCaml' if row['ocaml'] else 'Rust only'})
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
