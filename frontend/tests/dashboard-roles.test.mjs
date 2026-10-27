import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  blockFate,
  explorerNetwork,
  producerView,
  workerView,
} from '../src/app/features/dashboard/dashboard-roles/dashboard-roles.helper.ts';

// F-dash2: what the node does for the network. The "off" answers are the
// devnet node's (http://127.0.0.1:3600, main 915df1901) on 2026-10-23; the
// "on" answers follow the node's response structs (RpcBlockProducerStats,
// RpcSnarkWorkerControl, RpcIncludedWork), with public keys from the repo's
// tests.
const now = Date.UTC(2026, 9, 23, 1, 30, 0);
const ns = ms => ms * 1_000_000;
const min = 60_000;

const liveControlOff = {
  account_creation_fee: 1000000000,
  account_exists: null,
  allowed: true,
  best_tip_height: 578600,
  enabled: false,
  fee: null,
  fee_problem: null,
  job: null,
  paused_for_block: false,
  public_key: null,
  recent: [],
  setting_warning: null,
  worker: { kind: 'None' },
  zkapp_jobs: 41,
};
const liveIncludedNone = {
  network: 'devnet',
  keys: [],
  started_ms: null,
  final_totals: { blocks: 0, bundles: 0, zkapp_bundles: 0, fees: 0 },
  pending_totals: { blocks: 0, bundles: 0, zkapp_bundles: 0, fees: 0 },
  max_blocks: 500,
  entries: [],
};

const PRODUCER = 'B62qrzgDHmFmnuq6HSLBd7kERXepvfKcoBqTRS4JcBCMETcdFk4rwG9';
const WORKER = 'B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX';
const slot = (global_slot, offsetMs, epoch = 8) => ({
  slot_time: ns(now + offsetMs),
  global_slot,
  epoch,
  delegator: [PRODUCER, 12],
  value_with_threshold: [0.001, 0.002],
});
const stats = {
  current_time: ns(now),
  current_global_slot: 61740,
  current_epoch: 8,
  epoch_start: 57120,
  epoch_end: 64260,
  public_key: PRODUCER,
  attempts: [
    {
      won_slot: slot(61600, -200 * min),
      block: { hash: '3NKfirst', height: 578500 },
      times: {},
      status: 'Canonical',
      last_observed_confirmations: 98,
    },
    {
      won_slot: slot(61790, 75 * min),
      block: null,
      times: {},
      status: 'Scheduled',
    },
  ],
  future_won_slots: [slot(62100, 540 * min), slot(64300, 3000 * min, 9)],
  current_epoch_vrf_stats: { evaluated_slots: 7140, total_slots: 7140 },
  vrf_stats: {},
};

test('a node without a producer key shows the producer off', () => {
  // The devnet node's /status: "block_producer": null.
  const p = producerView(false, null, null, now);
  assert.equal(p.label, 'Off');
  assert.equal(p.on, false);
  assert.equal(p.tone, 'off');
  assert.equal(p.publicKey, null);
  assert.match(p.note, /without a producer key/);
  // An older node that does not say, and has no stats: off as well.
  assert.equal(producerView(undefined, null, null, now).label, 'Off');
});

test('a producer counts the won slots ahead in this epoch only', () => {
  const p = producerView(true, stats, null, now);
  assert.equal(p.label, 'On');
  assert.equal(p.tone, 'ok');
  assert.equal(p.publicKey, PRODUCER);
  // The scheduled attempt (61790) and 62100 are in epoch 8; 64300 is in 9.
  assert.equal(p.wonAhead, 2);
  assert.equal(p.wonAheadNote, 'epoch 8');
  assert.deepEqual(p.next, {
    at: now + 75 * min,
    globalSlot: 61790,
    epoch: 8,
  });
  assert.deepEqual(p.lastBlock, {
    height: 578500,
    hash: '3NKfirst',
    fate: 'canonical, 98 confirmations',
    tone: 'ok',
  });
  assert.equal(p.note, null);
});

test('won slots before the VRF has checked the epoch say so', () => {
  const partial = {
    ...stats,
    current_epoch_vrf_stats: { evaluated_slots: 3200, total_slots: 7140 },
  };
  assert.equal(
    producerView(true, partial, null, now).wonAheadNote,
    'epoch 8, 3,200 of 7,140 slots checked so far',
  );
});

test('a slot that has started is being produced, not ahead', () => {
  const later = now + 76 * min;
  const p = producerView(true, stats, null, later);
  assert.equal(p.label, 'Producing a block');
  assert.equal(p.wonAhead, 1);
  assert.equal(p.next.globalSlot, 62100);
  const proving = {
    ...stats,
    attempts: [
      stats.attempts[0],
      {
        ...stats.attempts[1],
        status: 'ProofCreatePending',
        block: { hash: '3NKsecond', height: 578640 },
      },
    ],
  };
  const q = producerView(true, proving, null, later);
  assert.equal(q.label, 'Producing a block');
  assert.equal(q.lastBlock.height, 578640);
  assert.equal(q.lastBlock.fate, 'being proved and applied');
});

test('a won slot that made no block is named, the last block kept', () => {
  const missed = {
    ...stats,
    attempts: [
      stats.attempts[0],
      {
        ...stats.attempts[1],
        won_slot: slot(61700, -20 * min),
        status: 'Discarded',
        discard_reason: 'BestTipSuperior',
      },
    ],
  };
  const p = producerView(true, missed, null, now);
  assert.equal(p.lastBlock.height, 578500);
  assert.equal(
    p.note,
    'Won slot 61700 made no block: a better block won the slot.',
  );
  assert.equal(p.noteWarn, true);
});

test('produced blocks get their fate in words', () => {
  const fate = (status, extra = {}) =>
    blockFate({ won_slot: slot(1, 0), status, ...extra });
  assert.deepEqual(fate('Canonical', { last_observed_confirmations: 1 }), {
    fate: 'canonical, 1 confirmation',
    tone: 'ok',
  });
  assert.equal(
    fate('Canonical', { last_observed_confirmations: 290 }).fate,
    'final',
  );
  assert.equal(fate('Committed').fate, 'our best tip, not yet confirmed');
  assert.deepEqual(fate('Orphaned', { orphaned_by: '3NKother' }), {
    fate: 'orphaned: another block took its place',
    tone: 'warn',
  });
  assert.equal(
    fate('Discarded', { discard_reason: 'ProofGenerationFailed' }).fate,
    'discarded: its proof failed',
  );
});

test('a producer whose stats are not ready, or fail, says why', () => {
  const waiting = producerView(true, null, null, now);
  assert.equal(waiting.label, 'On');
  assert.equal(waiting.tone, 'aware');
  assert.match(waiting.note, /not known yet/);
  const failed = producerView(true, null, 'The node did not answer.', now);
  assert.equal(failed.label, 'Unknown');
  assert.equal(failed.note, 'The node did not answer.');
  assert.equal(failed.noteWarn, true);
});

test('the live devnet worker is off, and can be turned on', () => {
  const w = workerView(liveControlOff, liveIncludedNone, null, now);
  assert.equal(w.label, 'Off');
  assert.equal(w.on, false);
  assert.equal(w.tone, 'off');
  assert.equal(w.includedBundles, 0);
  assert.equal(
    w.note,
    'Not proving. It can be turned on from SNARKs → Worker.',
  );
  assert.equal(w.noteWarn, false);
  // Without the Worker page for this node, no pointer to it.
  assert.equal(
    workerView(liveControlOff, liveIncludedNone, null, now, false).note,
    'Not proving.',
  );
  assert.match(
    workerView({ ...liveControlOff, allowed: false }, null, null, now).note,
    /does not allow worker control/,
  );
  // No answer yet: nothing to show.
  assert.equal(workerView(null, null, null, now), null);
});

test('a proving worker shows its key, fee, job, proofs and included work', () => {
  const control = {
    ...liveControlOff,
    enabled: true,
    public_key: WORKER,
    fee: 4_000_000,
    worker: { kind: 'Working', job_id: '1:2' },
    job: {
      job_id: '1:2',
      summary: { Merge: 3 },
      zkapp: false,
      submitted: ns(now - 42_000),
    },
    proofs_made: 23,
  };
  const included = {
    ...liveIncludedNone,
    keys: [WORKER],
    final_totals: { blocks: 5, bundles: 9, zkapp_bundles: 1, fees: 36_000_000 },
    pending_totals: {
      blocks: 1,
      bundles: 2,
      zkapp_bundles: 0,
      fees: 8_000_000,
    },
  };
  const w = workerView(control, included, null, now);
  assert.equal(w.label, 'Proving');
  assert.equal(w.tone, 'ok');
  assert.equal(w.publicKey, WORKER);
  assert.equal(w.fee, 4_000_000);
  assert.equal(w.job, 'merge job, 42 s');
  assert.equal(w.proofsMade, 23);
  assert.equal(w.includedBundles, 11);
  assert.equal(w.includedFees, 44_000_000);
  assert.equal(w.includedBlocks, 6);
  assert.equal(w.pendingBundles, 2);
  assert.equal(w.note, null);

  const idle = workerView(
    { ...control, worker: { kind: 'Idle' }, job: null, paused_for_block: true },
    included,
    null,
    now,
  );
  assert.equal(idle.label, 'Waiting for a job');
  assert.equal(idle.job, null);
  assert.equal(idle.jobNote, 'paused: a block of ours is due');
  // A node from before F-dash2 does not report proofs made.
  const { proofs_made, ...older } = control;
  assert.equal(workerView(older, included, null, now).proofsMade, null);
});

test('a fee OCaml would refuse, or no worker control, is a warning', () => {
  const refused = workerView(
    {
      ...liveControlOff,
      enabled: true,
      public_key: WORKER,
      fee: 4_000_000,
      worker: { kind: 'Idle' },
      fee_problem:
        "this account doesn't exist yet: fee must be at least 1 MINA, or fund it first",
    },
    null,
    null,
    now,
  );
  assert.equal(refused.noteWarn, true);
  assert.match(refused.note, /at least 1 MINA/);
  assert.equal(refused.includedBundles, null);
  const old = workerView(null, null, 'This node has no worker control.', now);
  assert.equal(old.label, 'Unknown');
  assert.equal(old.tone, 'warn');
});

test('explorer links only for a network the explorer has', () => {
  assert.equal(explorerNetwork('devnet'), 'devnet');
  assert.equal(explorerNetwork('Mainnet'), 'mainnet');
  assert.equal(explorerNetwork('Unknown'), null);
  assert.equal(explorerNetwork(undefined), null);
});
