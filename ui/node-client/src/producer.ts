/** Block production: `GET /stats/block_producer` (null without a producer). */

import { arr, nanosToMs, num, obj, str } from "./json";
import type { Tone } from "./sync";

export interface WonSlot {
  slotTimeMs: number | null;
  globalSlot: number | null;
  epoch: number | null;
  delegator: string | null;
}

export interface ProducedBlock {
  hash: string | null;
  height: number | null;
  payments: number;
  delegations: number;
  zkapps: number;
  works: number;
  /** Nanomina. */
  coinbase: number | null;
  fees: number | null;
  snarkFees: number | null;
}

/** When each stage of an attempt happened (ms since the Unix epoch), null
 * for a stage it didn't reach. */
export interface AttemptTimes {
  scheduled: number | null;
  stagedLedgerDiffStart: number | null;
  stagedLedgerDiffEnd: number | null;
  produced: number | null;
  proofStart: number | null;
  proofEnd: number | null;
  applyStart: number | null;
  applyEnd: number | null;
  committed: number | null;
  discarded: number | null;
}

export interface Attempt {
  wonSlot: WonSlot;
  block: ProducedBlock | null;
  status: string;
  confirmations: number | null;
  orphanedBy: string | null;
  discardReason: string | null;
  times: AttemptTimes;
}

/** One epoch's search for won slots (`vrf_stats`). The stake fields and
 * `wonSlots` are null from nodes that don't send them. */
export interface VrfEpoch {
  epoch: number;
  /** The slots to check: the whole epoch, or what was left of it when the
   * node started checking. */
  totalSlots: number;
  evaluatedSlots: number;
  wonSlots: number | null;
  /** Nanomina: the key's stake in the epoch's staking ledger, its own and
   * its delegators', and that ledger's total. */
  delegatedStake: number | null;
  totalCurrency: number | null;
  delegators: number | null;
}

export interface ProducerStats {
  publicKey: string | null;
  currentTimeMs: number | null;
  currentGlobalSlot: number | null;
  currentEpoch: number | null;
  epochStart: number | null;
  epochEnd: number | null;
  /** Newest first. */
  attempts: Attempt[];
  /** Soonest first. */
  futureWonSlots: WonSlot[];
  vrf: { evaluated: number; total: number } | null;
  /** Every epoch the node keeps the search of, oldest first. */
  vrfEpochs: VrfEpoch[];
}

function wonSlot(value: unknown): WonSlot {
  const w = obj(value) ?? {};
  const delegator = arr(w.delegator);
  return {
    slotTimeMs: nanosToMs(w.slot_time),
    globalSlot: num(w.global_slot),
    epoch: num(w.epoch),
    delegator: str(delegator[0]),
  };
}

function producedBlock(value: unknown): ProducedBlock | null {
  const b = obj(value);
  if (!b) return null;
  const txs = obj(b.transactions) ?? {};
  return {
    hash: str(b.hash),
    height: num(b.height),
    payments: num(txs.payments) ?? 0,
    delegations: num(txs.delegations) ?? 0,
    zkapps: num(txs.zkapps) ?? 0,
    works: num(b.completed_works_count) ?? 0,
    coinbase: num(b.coinbase),
    fees: num(b.fees),
    snarkFees: num(b.snark_fees),
  };
}

function attemptTimes(value: unknown): AttemptTimes {
  const t = obj(value) ?? {};
  return {
    scheduled: nanosToMs(t.scheduled),
    stagedLedgerDiffStart: nanosToMs(t.staged_ledger_diff_create_start),
    stagedLedgerDiffEnd: nanosToMs(t.staged_ledger_diff_create_end),
    produced: nanosToMs(t.produced),
    proofStart: nanosToMs(t.proof_create_start),
    proofEnd: nanosToMs(t.proof_create_end),
    applyStart: nanosToMs(t.block_apply_start),
    applyEnd: nanosToMs(t.block_apply_end),
    committed: nanosToMs(t.committed),
    discarded: nanosToMs(t.discarded),
  };
}

export function parseAttempt(value: unknown): Attempt | null {
  const a = obj(value);
  if (!a) return null;
  return {
    wonSlot: wonSlot(a.won_slot),
    block: producedBlock(a.block),
    status: str(a.status) ?? "Unknown",
    confirmations: num(a.last_observed_confirmations),
    orphanedBy: str(a.orphaned_by),
    discardReason: str(a.discard_reason),
    times: attemptTimes(a.times),
  };
}

function vrfEpochs(value: unknown): VrfEpoch[] {
  const epochs = obj(value) ?? {};
  return Object.entries(epochs)
    .map(([key, v]) => {
      const e = obj(v);
      const epoch = num(key);
      const totalSlots = num(e?.total_slots);
      const evaluatedSlots = num(e?.evaluated_slots);
      if (!e || epoch === null || totalSlots === null) return null;
      return {
        epoch,
        totalSlots,
        evaluatedSlots: evaluatedSlots ?? 0,
        wonSlots: num(e.won_slots),
        delegatedStake: num(e.delegated_stake),
        totalCurrency: num(e.total_currency),
        delegators: num(e.delegators),
      };
    })
    .filter((e): e is VrfEpoch => e !== null)
    .sort((a, b) => a.epoch - b.epoch);
}

/** Null when the node produces no blocks (the route answers `null`). */
export function parseProducerStats(raw: unknown): ProducerStats | null {
  const s = obj(raw);
  if (!s) return null;
  const vrf = obj(s.current_epoch_vrf_stats);
  const evaluated = num(vrf?.evaluated_slots);
  const total = num(vrf?.total_slots);
  return {
    publicKey: str(s.public_key),
    currentTimeMs: nanosToMs(s.current_time),
    currentGlobalSlot: num(s.current_global_slot),
    currentEpoch: num(s.current_epoch),
    epochStart: num(s.epoch_start),
    epochEnd: num(s.epoch_end),
    attempts: arr(s.attempts)
      .map(parseAttempt)
      .filter((a): a is Attempt => a !== null)
      .reverse(),
    futureWonSlots: arr(s.future_won_slots)
      .map(wonSlot)
      .sort((a, b) => (a.globalSlot ?? 0) - (b.globalSlot ?? 0)),
    vrf: evaluated !== null && total !== null ? { evaluated, total } : null,
    vrfEpochs: vrfEpochs(s.vrf_stats),
  };
}

const discardReasons: Record<string, string> = {
  BestTipStakingLedgerDifferent: "the best tip's staking ledger changed",
  BestTipGlobalSlotHigher: "the best tip is past the slot",
  BestTipSuperior: "a better block won the slot",
  ProofVerificationFailed: "its proof didn't verify",
  ProofGenerationFailed: "its proof couldn't be made",
  WonSlotExpired: "the slot ended before the block was started",
};

export interface AttemptView {
  label: string;
  tone: Tone;
  detail: string | null;
}

export function attemptView(a: Attempt): AttemptView {
  switch (a.status) {
    case "Missed":
      return {
        label: "Missed",
        tone: "warn",
        detail: "the slot passed after a restart without another attempt",
      };
    case "Scheduled":
      return { label: "Scheduled", tone: "info", detail: null };
    case "Committed":
      return {
        label: "Committed",
        tone: "info",
        detail: "waiting for confirmations",
      };
    case "Canonical":
      return {
        label: "Canonical",
        tone: "ok",
        detail:
          a.confirmations !== null ? `${a.confirmations} confirmations` : null,
      };
    case "Orphaned":
      return {
        label: "Orphaned",
        tone: "warn",
        detail: "another block took its place",
      };
    case "Discarded":
      return {
        label: "Discarded",
        tone: "warn",
        detail: a.discardReason
          ? (discardReasons[a.discardReason] ?? a.discardReason)
          : null,
      };
    default:
      return { label: "Producing", tone: "info", detail: a.status };
  }
}

/** A won slot ahead; `scheduled` once the node has scheduled its attempt. */
export interface UpcomingSlot extends WonSlot {
  scheduled: boolean;
}

/**
 * The won slots ahead, soonest first. Once the node schedules the attempt
 * for its next won slot, that slot leaves `futureWonSlots` and is an attempt
 * with status `Scheduled`: it is still ahead, so it comes first here. Pass
 * when the node's latest run began (`startedMs`) to leave out attempts the
 * run before a restart scheduled: this run schedules its own.
 */
export function upcomingWonSlots(
  stats: ProducerStats,
  startedMs: number | null = null,
): UpcomingSlot[] {
  const scheduled = stats.attempts
    .filter(
      a =>
        a.status === "Scheduled" &&
        !(
          startedMs !== null &&
          a.times.scheduled !== null &&
          a.times.scheduled < startedMs
        ),
    )
    .map(a => ({ ...a.wonSlot, scheduled: true }));
  const seen = new Set(scheduled.map(w => w.globalSlot));
  const rest = stats.futureWonSlots
    .filter(w => !seen.has(w.globalSlot))
    .map(w => ({ ...w, scheduled: false }));
  return [...scheduled, ...rest].sort(
    (a, b) =>
      (a.slotTimeMs ?? Number.MAX_SAFE_INTEGER) -
      (b.slotTimeMs ?? Number.MAX_SAFE_INTEGER),
  );
}
