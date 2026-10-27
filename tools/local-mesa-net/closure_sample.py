#!/usr/bin/env python3
"""Record sustained private-pair ledger, account and work observations."""
import argparse
from collections import Counter
import datetime
import gzip
import json
import os
from pathlib import Path
import time

from processes import track
from sample import fetch

BLOCK = """stateHash creator protocolState { previousStateHash
 consensusState { blockHeight slotSinceGenesis }
 blockchainState { snarkedLedgerHash stagedLedgerHash stagedLedgerAuxHash
 stagedLedgerPendingCoinbaseAux stagedLedgerPendingCoinbaseHash
 stagedLedgerProofEmitted } }
 transactions { coinbase userCommands { hash fee amount nonce from to failureReason }
 feeTransfer { recipient fee type } } snarkJobs { fee prover workIds }"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dir', type=Path, required=True)
    args = parser.parse_args()
    work = args.dir.resolve()
    assert os.readlink('/proc/self/ns/net') == json.loads((work/'namespace.json').read_text())['netns']
    (work/'sample.pid').write_text(str(os.getpid())+'\n')
    accounts = {name: (work/'keys'/f'{name}.pub').read_text().strip()
                for name in ('producer', 'sender', 'receiver', 'small')}
    query = '{ bestChain(maxLength: 5) { '+BLOCK+' } '+ ' '.join(
        f'{name}: account(publicKey: {json.dumps(key)}) {{ publicKey nonce balance {{ total }} }}'
        for name, key in accounts.items()) + ' }'
    (work/'observation-query.graphql').write_text(query+'\n')
    (work/'scan').mkdir(exist_ok=True)
    seen = set()
    accepted = {}
    with (work/'samples.jsonl').open('a', buffering=1) as output:
        while True:
            track(work)
            row = {'time': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                   'rust_status': fetch('http://127.0.0.1:3660/status'),
                   'producer': fetch('http://127.0.0.1:3660/stats/block_producer'),
                   'rust': fetch('http://127.0.0.1:3660/graphql', query),
                   'ocaml': fetch('http://127.0.0.1:3670/graphql', query)}
            chain = (row['ocaml'].get('data') or {}).get('bestChain') or []
            for block in chain:
                accepted.setdefault(block['stateHash'], block)
            rust_chain = (row['rust'].get('data') or {}).get('bestChain') or []
            if rust_chain:
                tip = max(rust_chain, key=lambda b: int(b['protocolState']['consensusState']['blockHeight']))
                state_hash = tip['stateHash']
                if state_hash not in seen:
                    scan = fetch('http://127.0.0.1:3660/scan-state/summary/'+state_hash)
                    if 'scan_state' in scan:
                        with gzip.open(work/'scan'/f'{state_hash}.json.gz', 'wt') as f:
                            json.dump(scan, f)
                        row['scan_summary'] = {'hash': state_hash, 'trees': len(scan['scan_state']),
                            'jobs': dict(Counter(job['status'] for tree in scan['scan_state'] for job in tree))}
                        seen.add(state_hash)
            output.write(json.dumps(row)+'\n')
            blocks = sorted(accepted.values(), key=lambda b: int(b['protocolState']['consensusState']['blockHeight']))
            roots = [b['protocolState']['blockchainState']['snarkedLedgerHash'] for b in blocks]
            summary = {'time': row['time'], 'accepted_blocks_including_genesis': len(blocks),
                       'snarked_ledger_advances': sum(a != b for a, b in zip(roots, roots[1:])),
                       'works_bought': sum(len(b['snarkJobs']) for b in blocks),
                       'transactions': sum(len(b['transactions']['userCommands']) for b in blocks),
                       'tip': blocks[-1]['stateHash'] if blocks else None,
                       'sender_nonce': ((row['rust'].get('data') or {}).get('sender') or {}).get('nonce'),
                       'rust_status': row['rust_status'],
                       'graphql_errors': {n: row[n].get('errors', row[n].get('sample_error')) for n in ('rust', 'ocaml')}}
            tmp = work/'live-summary.tmp'
            tmp.write_text(json.dumps(summary, indent=2)+'\n')
            tmp.replace(work/'live-summary.json')
            time.sleep(10)


if __name__ == '__main__':
    main()
