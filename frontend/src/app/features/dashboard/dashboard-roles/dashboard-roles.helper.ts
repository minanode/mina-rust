/**
 * What the node does for the network (F-dash2): whether it produces blocks
 * and whether it proves SNARK work, with the key facts for each. Pure
 * functions over the node's answers, so that they can be tested without
 * Angular (frontend/tests/dashboard-roles.test.mjs).
 */
import type {
  IncludedWork,
  WorkerControl,
} from '../../snarks/worker/snarks-worker.types';

/** The protocol's k: a block this deep in the best chain is final. */
export const FINAL_CONFIRMATIONS = 290;

export type RoleTone = 'ok' | 'aware' | 'warn' | 'off';

/** The node's `GET /stats/block_producer`, the fields used here. */
export interface ProducerStats {
  /** Nanoseconds since the Unix epoch. */
  current_time: number;
  current_epoch: number | null;
  public_key: string;
  /** Oldest first. */
  attempts: ProducerAttempt[];
  /** Won slots after the last attempt's slot. */
  future_won_slots: ProducerWonSlot[];
  current_epoch_vrf_stats: {
    evaluated_slots: number;
    total_slots: number;
  } | null;
}

export interface ProducerWonSlot {
  /** Nanoseconds since the Unix epoch. */
  slot_time: number;
  global_slot: number;
  epoch: number;
}

export interface ProducerAttempt {
  won_slot: ProducerWonSlot;
  block?: { hash: string; height: number } | null;
  status: string;
  last_observed_confirmations?: number;
  orphaned_by?: string;
  discard_reason?: string;
}

export interface ProducedBlockView {
  height: number;
  hash: string;
  fate: string;
  tone: RoleTone;
}

export interface ProducerView {
  /** 'Off', 'On', 'Producing a block', 'Unknown'. */
  label: string;
  tone: RoleTone;
  on: boolean;
  publicKey: string | null;
  /** Won slots still ahead in the current epoch; null when unknown. */
  wonAhead: number | null;
  /** e.g. 'epoch 14', 'epoch 14, 3,200 of 7,140 slots checked so far'. */
  wonAheadNote: string;
  /** The next won slot, in any epoch the node has evaluated. */
  next: { at: number; globalSlot: number; epoch: number } | null;
  /** The latest block this node produced, as far as it knows. */
  lastBlock: ProducedBlockView | null;
  note: string | null;
  /** The note is a problem, not an explanation. */
  noteWarn: boolean;
}

export interface WorkerView {
  /** 'Off', 'Proving', 'Waiting for a job', 'Starting', ... */
  label: string;
  tone: RoleTone;
  on: boolean;
  publicKey: string | null;
  /** In nanomina. */
  fee: number | null;
  /** e.g. 'merge job, 42 s', or null when it has none. */
  job: string | null;
  /** Shown instead of a job: 'paused: a block of ours is due', 'none'. */
  jobNote: string;
  /** Since the node started; null from a node that does not report it. */
  proofsMade: number | null;
  /** Bundles of our work that blocks included, final and not yet final. */
  includedBundles: number | null;
  /** Their fees, in nanomina. */
  includedFees: number | null;
  includedBlocks: number | null;
  /** Of includedBundles, those not yet final. */
  pendingBundles: number;
  note: string | null;
  /** The note is a problem, not an explanation. */
  noteWarn: boolean;
}

const IN_PROGRESS = new Set([
  'StagedLedgerDiffCreatePending',
  'StagedLedgerDiffCreateSuccess',
  'Produced',
  'ProofCreatePending',
  'ProofCreateSuccess',
  'BlockApplyPending',
  'BlockApplySuccess',
]);

const DISCARD_REASONS: Record<string, string> = {
  BestTipStakingLedgerDifferent: 'the staking ledger changed',
  BestTipGlobalSlotHigher: 'the chain had passed the slot',
  BestTipSuperior: 'a better block won the slot',
  ProofGenerationFailed: 'its proof failed',
  ProofVerificationFailed: 'its proof did not verify',
  WonSlotExpired: 'the slot passed before it was ready',
};

function discardReason(reason: string | undefined): string {
  return (reason && DISCARD_REASONS[reason]) ?? 'discarded';
}

const count = (n: number): string => n.toLocaleString('en-US');
const nsToMs = (ns: number): number => Math.floor(ns / 1_000_000);

/** A produced block's fate, as the node last saw it. */
export function blockFate(attempt: ProducerAttempt): {
  fate: string;
  tone: RoleTone;
} {
  switch (attempt.status) {
    case 'Canonical': {
      const c = attempt.last_observed_confirmations ?? 0;
      if (c >= FINAL_CONFIRMATIONS) {
        return { fate: 'final', tone: 'ok' };
      }
      return {
        fate: `canonical, ${count(c)} confirmation${c === 1 ? '' : 's'}`,
        tone: 'ok',
      };
    }
    case 'Committed':
      return { fate: 'our best tip, not yet confirmed', tone: 'aware' };
    case 'Orphaned':
      return { fate: 'orphaned: another block took its place', tone: 'warn' };
    case 'Discarded':
      return {
        fate: `discarded: ${discardReason(attempt.discard_reason)}`,
        tone: 'warn',
      };
    default:
      return IN_PROGRESS.has(attempt.status)
        ? { fate: 'being proved and applied', tone: 'aware' }
        : { fate: attempt.status, tone: 'aware' };
  }
}

/**
 * The block producer. `hasProducer` is /status's `block_producer` (null:
 * off; undefined: an older node that does not say). `stats` is null when the
 * node cannot report yet: no best tip, or its staking ledgers not evaluated.
 */
export function producerView(
  hasProducer: boolean | undefined,
  stats: ProducerStats | null,
  error: string | null,
  now: number,
): ProducerView {
  const off: ProducerView = {
    label: 'Off',
    tone: 'off',
    on: false,
    publicKey: null,
    wonAhead: null,
    wonAheadNote: '',
    next: null,
    lastBlock: null,
    note: null,
    noteWarn: false,
  };
  if (hasProducer === false || (!stats && hasProducer === undefined)) {
    return {
      ...off,
      note: error ?? 'Started without a producer key (--producer-key).',
      noteWarn: !!error,
    };
  }
  if (!stats) {
    return {
      ...off,
      label: error ? 'Unknown' : 'On',
      tone: error ? 'warn' : 'aware',
      on: true,
      note:
        error ??
        'Won slots are not known yet: the node needs a best tip and its staking ledger.',
      noteWarn: !!error,
    };
  }

  const epoch = stats.current_epoch;
  const scheduled = stats.attempts
    .filter(a => a.status === 'Scheduled')
    .map(a => a.won_slot);
  const ahead = [...scheduled, ...stats.future_won_slots]
    .filter(s => nsToMs(s.slot_time) > now)
    .sort((a, b) => a.slot_time - b.slot_time);
  const vrf = stats.current_epoch_vrf_stats;
  const partial = !!vrf && vrf.evaluated_slots < vrf.total_slots;
  const wonAheadNote =
    epoch === null
      ? ''
      : partial
        ? `epoch ${epoch}, ${count(vrf.evaluated_slots)} of ${count(vrf.total_slots)} slots checked so far`
        : `epoch ${epoch}`;

  const latest = stats.attempts[stats.attempts.length - 1];
  const producing =
    !!latest &&
    (IN_PROGRESS.has(latest.status) ||
      (latest.status === 'Scheduled' &&
        nsToMs(latest.won_slot.slot_time) <= now));

  const withBlock = [...stats.attempts].reverse().find(a => !!a.block);
  const lastBlock: ProducedBlockView | null = withBlock?.block
    ? {
        height: withBlock.block.height,
        hash: withBlock.block.hash,
        ...blockFate(withBlock),
      }
    : null;

  // A won slot that ended without a block since the last block: say so.
  const missed =
    latest && latest.status === 'Discarded' && !latest.block
      ? `Won slot ${latest.won_slot.global_slot} made no block: ${discardReason(latest.discard_reason)}.`
      : null;

  const next = ahead[0];
  return {
    label: producing ? 'Producing a block' : 'On',
    tone: 'ok',
    on: true,
    publicKey: stats.public_key,
    wonAhead:
      epoch === null ? null : ahead.filter(s => s.epoch === epoch).length,
    wonAheadNote,
    next: next
      ? {
          at: nsToMs(next.slot_time),
          globalSlot: next.global_slot,
          epoch: next.epoch,
        }
      : null,
    lastBlock,
    note: missed,
    noteWarn: !!missed,
  };
}

function jobKind(job: {
  zkapp: boolean;
  summary?: { Tx?: number; Merge?: number } | null;
}): string {
  if (job.zkapp) {
    return 'zkApp command';
  }
  if (!job.summary) {
    return 'bundle';
  }
  return job.summary.Merge !== undefined ? 'merge' : 'transaction';
}

function workerLabel(control: WorkerControl): {
  label: string;
  tone: RoleTone;
} {
  switch (control.worker.kind) {
    case 'None':
      return control.enabled
        ? { label: 'Starting soon', tone: 'aware' }
        : { label: 'Off', tone: 'off' };
    case 'Starting':
      return { label: 'Starting', tone: 'aware' };
    case 'Idle':
      return { label: 'Waiting for a job', tone: 'ok' };
    case 'Working':
      return { label: 'Proving', tone: 'ok' };
    case 'WorkReady':
    case 'WorkError':
    case 'Cancelled':
      return { label: 'Finishing a job', tone: 'ok' };
    case 'Cancelling':
      return { label: 'Cancelling a job', tone: 'aware' };
    case 'Killing':
      return { label: 'Stopping', tone: 'aware' };
    case 'Error':
      return { label: 'Stopped on an error', tone: 'warn' };
    default:
      return { label: String(control.worker.kind), tone: 'aware' };
  }
}

/** "42 s", "3 min", "2 h". */
function shortAge(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 120) {
    return `${s} s`;
  }
  const m = Math.round(s / 60);
  return m < 120 ? `${m} min` : `${Math.round(m / 60)} h`;
}

/**
 * The SNARK worker, from `GET /snarker/control` and `GET /snarker/included`
 * (P5.4). `control` is null until the first answer, or on an error.
 * `workerPage`: this dashboard has the SNARKs → Worker page for the node.
 */
export function workerView(
  control: WorkerControl | null,
  included: IncludedWork | null,
  error: string | null,
  now: number,
  workerPage = true,
): WorkerView | null {
  const totals = included
    ? {
        includedBundles:
          included.final_totals.bundles + included.pending_totals.bundles,
        includedFees: included.final_totals.fees + included.pending_totals.fees,
        includedBlocks:
          included.final_totals.blocks + included.pending_totals.blocks,
        pendingBundles: included.pending_totals.bundles,
      }
    : {
        includedBundles: null,
        includedFees: null,
        includedBlocks: null,
        pendingBundles: 0,
      };
  if (!control) {
    if (!error) {
      return null;
    }
    return {
      label: 'Unknown',
      tone: 'warn',
      on: false,
      publicKey: null,
      fee: null,
      job: null,
      jobNote: '',
      proofsMade: null,
      ...totals,
      note: error,
      noteWarn: true,
    };
  }
  const { label, tone } = workerLabel(control);
  const job = control.job
    ? `${jobKind(control.job)} job, ${shortAge(now - nsToMs(control.job.submitted))}`
    : null;
  const jobNote = control.paused_for_block
    ? 'paused: a block of ours is due'
    : 'none';
  let note: string | null = null;
  let noteWarn = false;
  if (!control.enabled) {
    note = !control.allowed
      ? 'Not proving. This node does not allow worker control from the dashboard.'
      : workerPage
        ? 'Not proving. It can be turned on from SNARKs → Worker.'
        : 'Not proving.';
  } else if (control.fee_problem) {
    note = control.fee_problem;
    noteWarn = true;
  } else if (control.worker.kind === 'Error') {
    note = 'The worker stopped on an error: see SNARKs → Worker.';
    noteWarn = true;
  }
  return {
    label,
    tone,
    on: control.enabled,
    publicKey: control.public_key,
    fee: control.fee,
    job,
    jobNote,
    proofsMade: control.proofs_made ?? null,
    ...totals,
    note,
    noteWarn,
  };
}

/** The explorer network for the node's network name, or null if neither. */
export function explorerNetwork(
  network: string | null | undefined,
): 'mainnet' | 'devnet' | null {
  const name = network?.toLowerCase();
  return name === 'mainnet' || name === 'devnet' ? name : null;
}
