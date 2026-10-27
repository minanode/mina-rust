#!/usr/bin/env python3
"""Summarise m9's P4.6c rerun, as F4's summarize_run.py did: blocks injected
and accepted, OCaml rejections ("too late" counted separately), every OCaml
block receipt with its slot against the slot it arrived in, genesis receipts,
and the Rust producer's own suppressed stale sends (P2pBlockGossipSuppressed,
debug level in this harness). Writes RUN/m9-summary.json."""
import datetime
import hashlib
import json
import os
import pathlib

work = pathlib.Path(os.environ['GATE_DIR']).resolve() / 'p46c/run1'
x = json.load(open(work / 'results.json'))
genesis = json.load(open(work / 'identity-match.json'))
genesis_time = datetime.datetime.fromisoformat(
    json.load(open(work / 'daemon.json'))['genesis']['genesis_state_timestamp'].replace('Z', '+00:00'))
received = []
for line in open(work / 'ocaml-netns/mina.log', errors='replace'):
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        continue
    if event.get('message') == 'Received a block from $sender':
        meta = event['metadata']
        consensus = meta['block']['protocol_state']['body']['consensus_state']
        timestamp = datetime.datetime.fromisoformat(event['timestamp'].replace('Z', '+00:00'))
        received.append({
            'hash': meta['state_hash'], 'timestamp': event['timestamp'],
            'slot': int(consensus['curr_global_slot_since_hard_fork']['slot_number']),
            'receive_slot': int((timestamp - genesis_time).total_seconds() * 1000) // 90000,
        })
suppressed = 0
for line in open(work / 'logs/rust-netns.log', errors='replace'):
    if 'P2pBlockGossipSuppressed' in line:
        suppressed += 1
blocks = [b for b in x['blocks'] if b['injected_after_state_machine_verification']]
summary = {
    'binary_sha256': hashlib.sha256(pathlib.Path(os.environ['MINA_BIN']).resolve().read_bytes()).hexdigest(),
    'identity_binary_sha256': hashlib.sha256(pathlib.Path(os.environ['IDENTITY_BIN']).resolve().read_bytes()).hexdigest(),
    'network_namespace': json.load(open(work / 'namespace.json')).get('netns'),
    'genesis_timestamp': genesis_time.isoformat(), 'genesis_hash': genesis.get('genesis_state_hash'),
    'chain_id': genesis.get('chain_id') or genesis.get('rust_chain_id'),
    'rust_blocks': x['rust_blocks'], 'ocaml_accepted_rust_blocks': x['ocaml_accepted_rust_blocks'],
    'service_self_verified': x.get('self_verified_rust_blocks'),
    'ocaml_rejections': len(x['ocaml_rejections']),
    'ocaml_too_late': sum((e.get('metadata') or {}).get('time_error') == 'too late' for e in x['ocaml_rejections']),
    'ocaml_genesis_receipts': sum(r['hash'] == genesis.get('genesis_state_hash') for r in received),
    'receipt_count': len(received), 'unique_received_hashes': len({r['hash'] for r in received}),
    'every_receipt_in_own_slot': all(r['slot'] == r['receive_slot'] for r in received),
    'every_injected_block_accepted': all(
        b['ocaml_frontier_log'] and b['ocaml_best_chain_first_seen'] and b['service_self_verified'] for b in blocks),
    'rust_suppressed_stale_sends': suppressed,
    'receipts': received,
    'blocks': [{k: b.get(k) for k in ['slot', 'height', 'hash', 'proof_seconds_including_self_check',
                                       'injected_after_state_machine_verification', 'ocaml_receipt_times',
                                       'ocaml_frontier_log', 'ocaml_best_chain_first_seen']} for b in blocks],
    'rejections': x['ocaml_rejections'],
}
(work / 'm9-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps({k: v for k, v in summary.items() if k not in ['blocks', 'receipts', 'rejections']}, indent=2))
for b in summary['blocks']:
    print(b['slot'], b['height'], b['hash'], b['proof_seconds_including_self_check'], bool(b['ocaml_frontier_log']),
          bool(b['ocaml_best_chain_first_seen']))
