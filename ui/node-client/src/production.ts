/**
 * What became of each won slot, in words, with its stages against the slot,
 * and the figures over many: outcomes, proof and broadcast times, coinbase,
 * and the wins to expect from the key's stake. Pure functions of
 * `ProducerStats`.
 */

import { formatDuration } from "./format";
import type {
  Attempt,
  ProducerStats,
  UpcomingSlot,
  VrfEpoch,
} from "./producer";
import type { Tone } from "./sync";
import { CONFIRMATIONS } from "./worker";

/** A slot's length on devnet and mainnet since the Mesa upgrade. */
export const SLOT_MS = 90_000;

/** Mina's active slot coefficient: a key with all the stake would win this
 * share of the slots (`f` in the consensus, 3/4). */
export const ACTIVE_SLOT_COEFFICIENT = 0.75;

/**
 * The slot length the node's own slot times show: two won slots' times over
 * the slots between them. `SLOT_MS` when it can't tell.
 */
export function slotDurationMs(stats: ProducerStats): number {
  const slots = [...stats.attempts.map(a => a.wonSlot), ...stats.futureWonSlots]
    .filter(w => w.globalSlot !== null && w.slotTimeMs !== null)
    .sort((a, b) => a.globalSlot! - b.globalSlot!);
  for (let i = 1; i < slots.length; i++) {
    const slotsBetween = slots[i].globalSlot! - slots[0].globalSlot!;
    if (slotsBetween > 0) {
      const ms = (slots[i].slotTimeMs! - slots[0].slotTimeMs!) / slotsBetween;
      return ms > 0 && Number.isFinite(ms) ? ms : SLOT_MS;
    }
  }
  return SLOT_MS;
}

/**
 * - `upcoming`: scheduled, the slot hasn't begun
 * - `producing`: being made now
 * - `broadcast`: sent, our best tip, nothing built on it yet (or the node
 *   restarted before it saw what became of it)
 * - `canonical`: in the best chain; labelled Confirmed from
 *   `CONFIRMATIONS` blocks on top, Pending below, as included SNARK work is
 * - `orphaned`: sent, then another block at its height took its place
 * - `lostTie`: another block for the same slot won the tie-break before
 *   ours was sent
 * - `missed`: the slot passed without our block (too late, a fork, a failed
 *   proof, or a restart)
 * - `previousRun`: scheduled, or begun, by the run before the node's latest
 *   restart, for a slot not over yet that this run hasn't scheduled again
 *   (it does when the slot is its next; this attempt then gives way)
 */
export type Outcome =
  | "upcoming"
  | "producing"
  | "previousRun"
  | "broadcast"
  | "canonical"
  | "orphaned"
  | "lostTie"
  | "missed";

export type StageKey =
  | "won"
  | "built"
  | "proofStart"
  | "proofEnd"
  | "injected"
  | "applied"
  | "broadcast";

/**
 * - `done`: it happened, at `atMs`
 * - `current`: under way
 * - `stopped`: where the attempt ended without it
 * - `skipped`: not reached, the attempt ended earlier
 * - `pending`: still to come
 */
export type StageState = "done" | "current" | "stopped" | "skipped" | "pending";

export interface Stage {
  key: StageKey;
  label: string;
  /** Ms since the Unix epoch. */
  atMs: number | null;
  /** Ms from the slot's start: negative before it. */
  offsetMs: number | null;
  state: StageState;
  detail: string | null;
}

export interface AttemptReport {
  attempt: Attempt;
  outcome: Outcome;
  label: string;
  tone: Tone;
  /** Why, in plain words; null when the label says it. */
  reason: string | null;
  slotStartMs: number | null;
  slotEndMs: number | null;
  stages: Stage[];
  /** The proof's time, start to end. */
  proofMs: number | null;
  /** When the block went out, from the slot's start. */
  broadcastOffsetMs: number | null;
  /** When the node dropped the attempt, from the slot's start. */
  discardedOffsetMs: number | null;
  /** The block reached peers: it was sent (broadcast, canonical or
   * orphaned). */
  sent: boolean;
  /** Left unfinished by a run before the node's latest start. */
  interrupted: boolean;
  /** Scheduled by a run before the node's latest start. */
  previousRun: boolean;
  /** Its outcome is settled: not upcoming, producing or from the run
   * before a restart. */
  ended: boolean;
}

export interface ReportContext {
  /** The node's clock (`ProducerStats.currentTimeMs`). */
  nowMs: number;
  /** When the node's latest run began (now less its uptime); null if not
   * known. */
  startedMs: number | null;
  slotMs?: number;
  /** The won slots this run has found ahead (`futureWonSlots`). */
  aheadSlots?: ReadonlySet<number>;
}

const DISCARD_REASONS: Record<
  string,
  { outcome: Outcome; tone: Tone; reason: string }
> = {
  BestTipSuperior: {
    outcome: "lostTie",
    tone: "warn",
    reason:
      "Another block for this slot won the tie-break (with the same key, the larger hash wins). Ours was never sent.",
  },
  BestTipGlobalSlotHigher: {
    outcome: "missed",
    tone: "warn",
    reason: "Too late: the chain had moved past this slot.",
  },
  WonSlotExpired: {
    outcome: "missed",
    tone: "warn",
    reason: "The slot ended before the block was ready.",
  },
  BestTipStakingLedgerDifferent: {
    outcome: "missed",
    tone: "warn",
    reason:
      "The chain switched to a fork with another staking ledger, where this slot isn't ours.",
  },
  ProofGenerationFailed: {
    outcome: "missed",
    tone: "error",
    reason: "The block's proof couldn't be made.",
  },
  ProofVerificationFailed: {
    outcome: "missed",
    tone: "error",
    reason: "The block's proof didn't verify.",
  },
};

const LABELS: Record<Outcome, string> = {
  upcoming: "Scheduled",
  producing: "Producing",
  previousRun: "From before the restart",
  broadcast: "Broadcast",
  canonical: "In the chain",
  orphaned: "Orphaned",
  lostTie: "Lost the tie",
  missed: "Missed",
};

const STAGES: { key: StageKey; label: string }[] = [
  { key: "won", label: "Won (VRF)" },
  { key: "built", label: "Built" },
  { key: "proofStart", label: "Proof started" },
  { key: "proofEnd", label: "Proof done" },
  { key: "injected", label: "Injected" },
  { key: "applied", label: "Applied" },
  { key: "broadcast", label: "Broadcast" },
];

function stageTime(a: Attempt, key: StageKey): number | null {
  const t = a.times;
  switch (key) {
    case "won":
      return t.scheduled;
    case "built":
      return t.produced;
    case "proofStart":
      return t.proofStart;
    case "proofEnd":
      return t.proofEnd;
    case "injected":
      return t.applyStart;
    case "applied":
      return t.applyEnd;
    case "broadcast":
      return t.committed;
  }
}

function span(from: number | null, to: number | null): number | null {
  return from !== null && to !== null && to >= from ? to - from : null;
}

function stageDetail(a: Attempt, key: StageKey): string | null {
  const t = a.times;
  const ms =
    key === "built"
      ? span(t.stagedLedgerDiffStart ?? t.stagedLedgerDiffEnd, t.produced)
      : key === "proofEnd"
        ? span(t.proofStart, t.proofEnd)
        : key === "applied"
          ? span(t.applyStart, t.applyEnd)
          : null;
  if (ms === null) return null;
  const took = ms < 50 ? "under 0.1 s" : formatDuration(ms);
  return key === "built" ? `staged ledger diff in ${took}` : `in ${took}`;
}

/** The attempt's stages, with where it got to. */
function stages(a: Attempt, slotStart: number | null, ended: boolean): Stage[] {
  const times = STAGES.map(s => stageTime(a, s.key));
  // The last stage it reached: the stages before it happened even if their
  // time is missing.
  let reached = -1;
  times.forEach((t, i) => {
    if (t !== null) reached = i;
  });
  return STAGES.map((s, i) => {
    const atMs = times[i];
    let state: StageState;
    if (atMs !== null || i < reached) state = "done";
    else if (i === reached + 1) state = ended ? "stopped" : "current";
    else state = ended ? "skipped" : "pending";
    return {
      key: s.key,
      label: s.label,
      atMs,
      offsetMs: atMs !== null && slotStart !== null ? atMs - slotStart : null,
      state,
      detail: stageDetail(a, s.key),
    };
  });
}

const PRODUCING = new Set([
  "StagedLedgerDiffCreatePending",
  "StagedLedgerDiffCreateSuccess",
  "Produced",
  "ProofCreatePending",
  "ProofCreateSuccess",
  "BlockApplyPending",
  "BlockApplySuccess",
]);

/** What became of a won slot, and how its stages went against the slot. */
export function attemptReport(a: Attempt, ctx: ReportContext): AttemptReport {
  const slotMs = ctx.slotMs ?? SLOT_MS;
  const slotStartMs = a.wonSlot.slotTimeMs;
  const slotEndMs = slotStartMs !== null ? slotStartMs + slotMs : null;
  // Scheduled by the run before the latest restart.
  const previousRun =
    ctx.startedMs !== null &&
    a.times.scheduled !== null &&
    a.times.scheduled < ctx.startedMs;
  const slotOver = slotEndMs !== null && slotEndMs <= ctx.nowMs;

  let outcome: Outcome;
  let tone: Tone;
  let reason: string | null = null;
  let interrupted = false;
  switch (a.status) {
    case "Missed":
      outcome = "missed";
      tone = "warn";
      interrupted = true;
      reason =
        "The slot passed after a restart without another attempt; no block was made.";
      break;
    case "Canonical":
      outcome = "canonical";
      tone = "ok";
      reason =
        a.confirmations === null
          ? "In the best chain."
          : a.confirmations >= CONFIRMATIONS
            ? `In the best chain with ${a.confirmations} blocks on top when last seen.`
            : `In the best chain with ${a.confirmations === 1 ? "1 block" : `${a.confirmations} blocks`} on top when last seen; confirmed at ${CONFIRMATIONS}.`;
      break;
    case "Orphaned":
      outcome = "orphaned";
      tone = "warn";
      reason = `Sent, then another block at its height${a.block?.height != null ? ` (#${a.block.height})` : ""} took its place in the chain.`;
      break;
    case "Committed":
      outcome = "broadcast";
      tone = "info";
      reason = previousRun
        ? "Sent; the node restarted before it saw what became of it. The explorer shows the block at its height."
        : "Sent and our best tip; no block on top of it yet.";
      break;
    case "Discarded": {
      const known = a.discardReason ? DISCARD_REASONS[a.discardReason] : null;
      outcome = known?.outcome ?? "missed";
      tone = known?.tone ?? "warn";
      reason =
        known?.reason ??
        (a.discardReason ? `Dropped: ${a.discardReason}.` : "Dropped.");
      break;
    }
    case "Scheduled":
      if (previousRun && slotOver) {
        outcome = "missed";
        tone = "warn";
        interrupted = true;
        reason =
          "The node restarted before the slot came and didn't schedule it again: no block was made.";
      } else if (previousRun) {
        outcome = "previousRun";
        tone = "neutral";
        reason = ctx.aheadSlots?.has(a.wonSlot.globalSlot ?? -1)
          ? "Scheduled before the node restarted. This run found the slot too, and schedules it when it's the next."
          : "Scheduled before the node restarted. This run hasn't found the slot among its won slots.";
      } else if (slotStartMs !== null && ctx.nowMs < slotStartMs) {
        outcome = "upcoming";
        tone = "info";
        reason = "The node waits for the slot, with the attempt ready.";
      } else {
        outcome = "producing";
        tone = "info";
        reason = "Starting on the block.";
      }
      break;
    default:
      if (PRODUCING.has(a.status) && !previousRun) {
        outcome = "producing";
        tone = "info";
        reason = "Under way.";
      } else if (PRODUCING.has(a.status) && !slotOver) {
        outcome = "previousRun";
        tone = "neutral";
        reason =
          "Begun before the node restarted, which stopped it. This run makes the block again if the slot is still its own.";
      } else {
        outcome = "missed";
        tone = "warn";
        interrupted = previousRun;
        reason = previousRun
          ? "The node restarted while making this block: it wasn't sent."
          : `The node reports it as ${a.status}.`;
      }
  }

  const ended =
    outcome !== "upcoming" &&
    outcome !== "producing" &&
    outcome !== "previousRun";
  const list = stages(a, slotStartMs, ended);
  return {
    attempt: a,
    outcome,
    label:
      outcome === "canonical"
        ? (a.confirmations ?? 0) >= CONFIRMATIONS
          ? "Confirmed"
          : "Pending"
        : LABELS[outcome],
    tone,
    reason,
    slotStartMs,
    slotEndMs,
    stages: list,
    proofMs: span(a.times.proofStart, a.times.proofEnd),
    broadcastOffsetMs:
      a.times.committed !== null && slotStartMs !== null
        ? a.times.committed - slotStartMs
        : null,
    discardedOffsetMs:
      a.times.discarded !== null && slotStartMs !== null
        ? a.times.discarded - slotStartMs
        : null,
    sent:
      outcome === "broadcast" ||
      outcome === "canonical" ||
      outcome === "orphaned",
    interrupted,
    previousRun,
    ended,
  };
}

/** The node's latest start, from its uptime and its clock. */
export function startedAt(
  stats: ProducerStats,
  uptimeSecs: number | null | undefined,
): number | null {
  return stats.currentTimeMs !== null &&
    uptimeSecs !== null &&
    uptimeSecs !== undefined
    ? stats.currentTimeMs - uptimeSecs * 1000
    : null;
}

/**
 * Every attempt's report, newest first, as the node lists them. An attempt
 * from before the latest restart that this run has scheduled again gives
 * way to this run's.
 */
export function attemptReports(
  stats: ProducerStats,
  uptimeSecs: number | null | undefined,
  nowMs: number = stats.currentTimeMs ?? Date.now(),
): AttemptReport[] {
  const ctx: ReportContext = {
    nowMs,
    startedMs: startedAt(stats, uptimeSecs),
    slotMs: slotDurationMs(stats),
    aheadSlots: new Set(
      stats.futureWonSlots
        .map(w => w.globalSlot)
        .filter((s): s is number => s !== null),
    ),
  };
  const reports = stats.attempts.map(a => attemptReport(a, ctx));
  const thisRun = new Set(
    reports.filter(r => !r.previousRun).map(r => r.attempt.wonSlot.globalSlot),
  );
  return reports.filter(
    r =>
      !(
        (r.outcome === "previousRun" ||
          r.attempt.status === "Scheduled" ||
          r.attempt.status === "Missed") &&
        r.previousRun &&
        thisRun.has(r.attempt.wonSlot.globalSlot)
      ),
  );
}

export interface Spread {
  median: number;
  worst: number;
  count: number;
}

function spread(values: (number | null)[]): Spread | null {
  const sorted = values
    .filter((v): v is number => v !== null)
    .sort((a, b) => a - b);
  if (sorted.length === 0) return null;
  const mid = Math.floor(sorted.length / 2);
  const median =
    sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
  return { median, worst: sorted[sorted.length - 1], count: sorted.length };
}

export interface ProductionSummary {
  /** Won slots that have come: every outcome but `upcoming` and
   * `previousRun`. */
  won: number;
  /** Won slots still ahead, the scheduled one included. */
  ahead: number;
  /** Sent: broadcast, canonical or orphaned. */
  produced: number;
  canonical: number;
  orphaned: number;
  lostTie: number;
  missed: number;
  producing: number;
  /** Proof start to end, over the attempts that finished a proof. */
  proof: Spread | null;
  /** Slot start to broadcast, over the blocks sent. */
  broadcast: Spread | null;
  /** Nanomina, over the canonical blocks: what went to the coinbase
   * receiver. */
  coinbase: number;
  fees: number;
  snarkFees: number;
  /** The oldest slot counted, ms since the Unix epoch. */
  sinceMs: number | null;
}

/** The figures over a set of attempts and the won slots ahead. */
export function productionSummary(
  reports: AttemptReport[],
  ahead: UpcomingSlot[],
): ProductionSummary {
  const count = (o: Outcome) => reports.filter(r => r.outcome === o).length;
  const canonical = reports.filter(r => r.outcome === "canonical");
  const sum = (pick: (r: AttemptReport) => number | null | undefined) =>
    canonical.reduce((total, r) => total + (pick(r) ?? 0), 0);
  const times = reports
    .map(r => r.slotStartMs)
    .filter((t): t is number => t !== null);
  // A scheduled attempt is both an attempt and a slot ahead: count it once.
  const upcomingSlots = new Set(
    reports
      .filter(r => r.outcome === "upcoming")
      .map(r => r.attempt.wonSlot.globalSlot),
  );
  return {
    won: reports.filter(
      r => r.outcome !== "upcoming" && r.outcome !== "previousRun",
    ).length,
    ahead:
      upcomingSlots.size +
      ahead.filter(w => !upcomingSlots.has(w.globalSlot)).length,
    produced: reports.filter(r => r.sent).length,
    canonical: canonical.length,
    orphaned: count("orphaned"),
    lostTie: count("lostTie"),
    missed: count("missed"),
    producing: count("producing"),
    proof: spread(reports.map(r => r.proofMs)),
    broadcast: spread(
      reports.filter(r => r.sent).map(r => r.broadcastOffsetMs),
    ),
    coinbase: sum(r => r.attempt.block?.coinbase),
    fees: sum(r => r.attempt.block?.fees),
    snarkFees: sum(r => r.attempt.block?.snarkFees),
    sinceMs: times.length ? Math.min(...times) : null,
  };
}

/** The chance the key wins a given slot, from its share of the stake:
 * 1 - (1 - f)^share. */
export function winProbability(stake: number, total: number): number {
  if (!(total > 0) || !(stake > 0)) return 0;
  return 1 - Math.pow(1 - ACTIVE_SLOT_COEFFICIENT, stake / total);
}

export interface EpochExpectation {
  epoch: number;
  /** Nanomina. */
  delegatedStake: number;
  totalCurrency: number;
  delegators: number | null;
  /** The key's share of the epoch's stake, 0 to 1. */
  share: number;
  /** The chance of winning each slot. */
  perSlot: number;
  slotsChecked: number;
  totalSlots: number;
  expected: number;
  /** The wins found fall from `low` to `high` 95 times in 100 by chance
   * alone: the central 95% of the binomial distribution of the slots
   * checked. Wide when few wins are expected. */
  low: number;
  high: number;
  /** Wins found in the slots checked. */
  found: number;
}

/**
 * Wins to expect against wins found, per epoch, for the epochs whose stake
 * the node reports (`vrf_stats` with the stake fields, newer builds). Empty
 * when it reports none.
 */
export function epochExpectations(epochs: VrfEpoch[]): EpochExpectation[] {
  return epochs.flatMap(e => {
    if (
      e.delegatedStake === null ||
      e.totalCurrency === null ||
      e.wonSlots === null ||
      !(e.totalCurrency > 0)
    ) {
      return [];
    }
    const perSlot = winProbability(e.delegatedStake, e.totalCurrency);
    const n = e.evaluatedSlots;
    const expected = n * perSlot;
    const [low, high] = binomialRange(n, perSlot);
    return [
      {
        epoch: e.epoch,
        delegatedStake: e.delegatedStake,
        totalCurrency: e.totalCurrency,
        delegators: e.delegators,
        share: e.delegatedStake / e.totalCurrency,
        perSlot,
        slotsChecked: n,
        totalSlots: e.totalSlots,
        expected,
        low,
        high,
        found: e.wonSlots,
      },
    ];
  });
}

/**
 * The central `mass` of the binomial distribution of wins in `n` slots, each
 * won with chance `p`: the narrowest `low` to `high` with P(X < low) and
 * P(X > high) each at most (1 - mass) / 2. Exact, and right for few slots
 * or few wins, where a normal approximation is not.
 */
export function binomialRange(
  n: number,
  p: number,
  mass = 0.95,
): [number, number] {
  if (!(n > 0) || !(p > 0)) return [0, 0];
  if (p >= 1) return [n, n];
  // The probabilities relative to the mode's, outwards until negligible, so
  // none underflows however many slots there are.
  const mode = Math.min(n, Math.floor((n + 1) * p));
  const odds = p / (1 - p);
  const up = [1];
  for (let k = mode, w = 1; k < n; k++) {
    w *= ((n - k) / (k + 1)) * odds;
    if (w < 1e-16) break;
    up.push(w);
  }
  const down: number[] = [];
  for (let k = mode, w = 1; k > 0; k--) {
    w *= k / (n - k + 1) / odds;
    if (w < 1e-16) break;
    down.push(w);
  }
  const from = mode - down.length;
  const weights = [...down.reverse(), ...up];
  const total = weights.reduce((a, b) => a + b, 0);
  const tail = ((1 - mass) / 2) * total;
  let below = 0;
  let low = from;
  for (let i = 0; i < weights.length; i++) {
    if (below + weights[i] > tail) {
      low = from + i;
      break;
    }
    below += weights[i];
  }
  let above = 0;
  let high = from + weights.length - 1;
  for (let i = weights.length - 1; i >= 0; i--) {
    if (above + weights[i] > tail) {
      high = from + i;
      break;
    }
    above += weights[i];
  }
  return [low, high];
}

/** Whether the node reports the stake behind its slot search at all. */
export function reportsStake(epochs: VrfEpoch[]): boolean {
  return epochs.some(
    e => e.delegatedStake !== null && e.totalCurrency !== null,
  );
}
