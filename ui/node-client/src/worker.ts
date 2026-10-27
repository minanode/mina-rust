/**
 * The SNARK worker: `GET /snarker/control`, `POST /snarker/control` (P5.4's
 * contract: JSON with the `x-mina-worker-control` header) and the
 * included-work log, `GET /snarker/included`.
 */

import { formatMina, minaToNanomina, plural } from "./format";
import { arr, bool, nanosToMs, num, obj, str } from "./json";
import type { Tone } from "./sync";

/** The header the node requires on a worker change. */
export const WORKER_CONTROL_HEADER = "x-mina-worker-control";

/** `--snarker-fee`'s default, in MINA. */
export const DEFAULT_FEE = "0.1";

export interface JobSummary {
  kind: "Tx" | "Merge" | null;
  /** Transactions in the job's scope. */
  size: number | null;
}

export interface WorkerJob {
  jobId: string | null;
  summary: JobSummary;
  zkapp: boolean;
  submittedMs: number | null;
}

export interface WorkerOutcome {
  jobId: string | null;
  summary: JobSummary;
  zkapp: boolean;
  timeMs: number | null;
  durationMs: number | null;
  /** Nanomina; null if the job failed. */
  fee: number | null;
  offered: boolean;
  error: string | null;
}

export interface WorkerControl {
  /** The node was started with --allow-worker-control. */
  allowed: boolean;
  enabled: boolean;
  publicKey: string | null;
  /** Nanomina. */
  fee: number | null;
  accountCreationFee: number | null;
  /** RpcSnarkWorkerStatus's `kind`. */
  workerKind: string;
  workerError: string | null;
  workerErrorPermanent: boolean;
  job: WorkerJob | null;
  recent: WorkerOutcome[];
  pausedForBlock: boolean;
  bestTipHeight: number | null;
  zkappJobs: number | null;
  /** Null when the node can't tell yet. */
  accountExists: boolean | null;
  feeProblem: string | null;
  settingWarning: string | null;
  proofsMade: number | null;
  /** How the worker picks its next job (`--snarker-strategy`); null from
   * an older node, which doesn't say. */
  strategy: SnarkerStrategy | null;
}

/** The node's names for `--snarker-strategy seq` and `rand`. */
export type SnarkerStrategy = "Sequential" | "Random";

/** How the worker picks jobs, in words, with the flag value that sets it. */
export function strategyLabel(strategy: SnarkerStrategy): string {
  return strategy === "Random"
    ? "At random (rand, the default)"
    : "In order (seq)";
}

function strategyOf(value: unknown): SnarkerStrategy | null {
  return value === "Random" || value === "Sequential" ? value : null;
}

export function jobSummary(value: unknown): JobSummary {
  const s = obj(value);
  if (s && num(s.Tx) !== null) return { kind: "Tx", size: num(s.Tx) };
  if (s && num(s.Merge) !== null) return { kind: "Merge", size: num(s.Merge) };
  return { kind: null, size: null };
}

export function describeJob(summary: JobSummary, zkapp: boolean): string {
  if (summary.kind === "Merge") return "Merge proof";
  if (summary.kind === "Tx")
    return zkapp ? "zkApp transaction proof" : "Transaction proof";
  return zkapp ? "zkApp job" : "Job";
}

/** An error value as the node serializes it: a string, or an enum such as
 * `{"Error": "..."}` or `"NotRunning"`. */
function errorText(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === "string") return value;
  const o = obj(value);
  if (o) {
    const [key, inner] = Object.entries(o)[0] ?? [];
    if (key === undefined) return null;
    const detail = typeof inner === "string" ? inner : errorText(inner);
    return detail ? `${key}: ${detail}` : key;
  }
  return JSON.stringify(value);
}

export function parseWorkerControl(raw: unknown): WorkerControl {
  const c = obj(raw) ?? {};
  const worker = obj(c.worker) ?? {};
  const job = obj(c.job);
  return {
    allowed: bool(c.allowed) ?? false,
    enabled: bool(c.enabled) ?? false,
    publicKey: str(c.public_key),
    fee: num(c.fee),
    accountCreationFee: num(c.account_creation_fee),
    workerKind: str(worker.kind) ?? "None",
    workerError: errorText(worker.error),
    workerErrorPermanent: bool(worker.permanent) ?? false,
    job: job
      ? {
          jobId: str(job.job_id),
          summary: jobSummary(job.summary),
          zkapp: bool(job.zkapp) ?? false,
          submittedMs: nanosToMs(job.submitted),
        }
      : null,
    recent: arr(c.recent).map(value => {
      const o = obj(value) ?? {};
      return {
        jobId: str(o.job_id),
        summary: jobSummary(o.summary),
        zkapp: bool(o.zkapp) ?? false,
        timeMs: nanosToMs(o.time),
        durationMs: num(o.duration_ms),
        fee: num(o.fee),
        offered: bool(o.offered) ?? false,
        error: str(o.error),
      };
    }),
    pausedForBlock: bool(c.paused_for_block) ?? false,
    bestTipHeight: num(c.best_tip_height),
    zkappJobs: num(c.zkapp_jobs),
    accountExists: bool(c.account_exists),
    feeProblem: str(c.fee_problem),
    settingWarning: str(c.setting_warning),
    proofsMade: num(c.proofs_made),
    strategy: strategyOf(c.strategy),
  };
}

export interface WorkerView {
  label: string;
  tone: Tone;
  detail: string;
}

/** The worker's state in words. */
export function workerView(c: WorkerControl): WorkerView {
  switch (c.workerKind) {
    case "Starting":
      return {
        label: "Starting",
        tone: "info",
        detail: "The prover process is starting.",
      };
    case "Idle":
      return c.pausedForBlock
        ? {
            label: "Paused: a block of ours is due",
            tone: "info",
            detail:
              "Waiting until our block slot is over before taking another job.",
          }
        : { label: "Idle", tone: "ok", detail: "Running, waiting for a job." };
    case "Working":
      return {
        label: "Working",
        tone: "ok",
        detail: c.job
          ? `Proving: ${describeJob(c.job.summary, c.job.zkapp).toLowerCase()}.`
          : "Proving a job.",
      };
    case "WorkReady":
      return {
        label: "Proof ready",
        tone: "ok",
        detail: "A proof is done and being offered.",
      };
    case "WorkError":
      return {
        label: "Job failed",
        tone: "warn",
        detail: "The last job failed; it takes the next.",
      };
    case "Cancelling":
    case "Cancelled":
      return {
        label: "Cancelling",
        tone: "info",
        detail: "Dropping a job that's no longer needed.",
      };
    case "Error":
      return {
        label: "Error",
        tone: "error",
        detail:
          (c.workerError ?? "The prover failed.") +
          (c.workerErrorPermanent ? " It won't restart by itself." : ""),
      };
    case "Killing":
      return {
        label: "Stopping",
        tone: "info",
        detail: "The prover process is stopping.",
      };
    default:
      return c.enabled
        ? {
            label: "Starting",
            tone: "info",
            detail: "The prover process will start shortly.",
          }
        : {
            label: "Off",
            tone: "neutral",
            detail: "This node isn't proving SNARK work.",
          };
  }
}

export const PUBLIC_KEY_PATTERN = /^B62[1-9A-HJ-NP-Za-km-z]{52}$/;

export interface WorkerChange {
  enabled: boolean;
  public_key?: string;
  fee?: string;
}

export type ChangeCheck =
  | { ok: true; change: WorkerChange }
  | { ok: false; field: "public_key" | "fee"; message: string };

/** Checks the form before it is sent; the node checks again, including the
 * fee rule for a key without an account. */
export function startChange(publicKey: string, fee: string): ChangeCheck {
  const key = publicKey.trim();
  const mina = fee.trim();
  if (!PUBLIC_KEY_PATTERN.test(key)) {
    return {
      ok: false,
      field: "public_key",
      message: "A public key starts with B62 and has 55 characters.",
    };
  }
  const nanomina = minaToNanomina(mina);
  if (nanomina === null) {
    return {
      ok: false,
      field: "fee",
      message:
        "The fee is an amount in MINA, like 0.1, with at most 9 decimals.",
    };
  }
  return { ok: true, change: { enabled: true, public_key: key, fee: mina } };
}

export const STOP_CHANGE: WorkerChange = { enabled: false };

/** The fee the form starts with: the worker's, or the default. */
export function feeInput(c: WorkerControl | null): string {
  return c?.fee !== null && c?.fee !== undefined
    ? (formatMina(c.fee) ?? DEFAULT_FEE).replace(/,/g, "")
    : DEFAULT_FEE;
}

export type IncludedStatus = "pending" | "final" | "dropped" | "unknown";

export interface IncludedTotals {
  blocks: number;
  bundles: number;
  zkappBundles: number;
  /** Nanomina. */
  fees: number;
}

export interface IncludedEntry {
  height: number;
  stateHash: string | null;
  timeMs: number | null;
  producer: string | null;
  prover: string | null;
  bundles: number;
  zkappBundles: number;
  fees: number;
  status: IncludedStatus;
}

export interface IncludedWork {
  network: string | null;
  keys: string[];
  startedMs: number | null;
  finalTotals: IncludedTotals;
  pendingTotals: IncludedTotals;
  maxBlocks: number | null;
  entries: IncludedEntry[];
  /** The best tip's height when the node read the log; null from a node
   * that doesn't send it (older builds). */
  bestTipHeight: number | null;
  /** What blocks buy from every prover; null from older builds. */
  market: SnarkMarket | null;
}

/** A best-chain block's purchase of SNARK work. Fees in nanomina. */
export interface SnarkPurchase {
  height: number;
  stateHash: string | null;
  timeMs: number | null;
  bundles: number;
  provers: number;
  minFee: number;
  maxFee: number;
}

/** One prover's completed work in the node's SNARK pool. */
export interface SnarkOffer {
  prover: string;
  bundles: number;
  minFee: number;
  maxFee: number;
}

/** The SNARK work market as the node sees it: what its best chain bought
 * and what its pool holds, from every prover. */
export interface SnarkMarket {
  /** Best-chain blocks looked at (the frontier, about 290). */
  blocks: number;
  buyingBlocks: number;
  bundles: number;
  /** The newest buying blocks, newest first. */
  purchases: SnarkPurchase[];
  /** Per prover, lowest fee first: the node's own keys and, from builds
   * that cap the list, the cheapest ten others. */
  offers: SnarkOffer[];
  /** Provers other than the node's own keys with work in the pool, listed
   * or not; null from builds that list them all. */
  otherProvers: number | null;
  /** The highest fee any of them asks; null from builds that list all. */
  highestOtherFee: number | null;
}

function parseMarket(value: unknown): SnarkMarket | null {
  const m = obj(value);
  if (!m) return null;
  return {
    blocks: num(m.blocks) ?? 0,
    buyingBlocks: num(m.buying_blocks) ?? 0,
    bundles: num(m.bundles) ?? 0,
    purchases: arr(m.purchases).flatMap(value => {
      const p = obj(value);
      const height = num(p?.height);
      if (!p || height === null) return [];
      return [
        {
          height,
          stateHash: str(p.state_hash),
          timeMs: num(p.time_ms),
          bundles: num(p.bundles) ?? 0,
          provers: num(p.provers) ?? 0,
          minFee: num(p.min_fee) ?? 0,
          maxFee: num(p.max_fee) ?? 0,
        },
      ];
    }),
    offers: arr(m.offers).flatMap(value => {
      const o = obj(value);
      const prover = str(o?.prover);
      if (!o || prover === null) return [];
      return [
        {
          prover,
          bundles: num(o.bundles) ?? 0,
          minFee: num(o.min_fee) ?? 0,
          maxFee: num(o.max_fee) ?? 0,
        },
      ];
    }),
    otherProvers: num(m.other_provers),
    highestOtherFee: num(m.highest_other_fee),
  };
}

function totals(value: unknown): IncludedTotals {
  const t = obj(value) ?? {};
  return {
    blocks: num(t.blocks) ?? 0,
    bundles: num(t.bundles) ?? 0,
    zkappBundles: num(t.zkapp_bundles) ?? 0,
    fees: num(t.fees) ?? 0,
  };
}

function includedStatus(value: unknown): IncludedStatus {
  const s = str(value);
  return s === "pending" || s === "final" || s === "dropped" ? s : "unknown";
}

export function parseIncluded(raw: unknown): IncludedWork {
  const w = obj(raw) ?? {};
  return {
    network: str(w.network),
    keys: arr(w.keys)
      .map(str)
      .filter((k): k is string => k !== null),
    startedMs: num(w.started_ms),
    finalTotals: totals(w.final_totals),
    pendingTotals: totals(w.pending_totals),
    maxBlocks: num(w.max_blocks),
    entries: arr(w.entries).flatMap(value => {
      const e = obj(value);
      const height = num(e?.height);
      if (!e || height === null) return [];
      return [
        {
          height,
          stateHash: str(e.state_hash),
          timeMs: num(e.time_ms),
          producer: str(e.producer),
          prover: str(e.prover),
          bundles: num(e.bundles) ?? 0,
          zkappBundles: num(e.zkapp_bundles) ?? 0,
          fees: num(e.fees) ?? 0,
          status: includedStatus(e.status),
        },
      ];
    }),
    bestTipHeight: num(w.best_tip_height),
    market: parseMarket(w.market),
  };
}

/**
 * Blocks on top of a block before the work it bought counts as confirmed:
 * Mina's recommended wait for a payment, "15 blocks which provides a 99.9%
 * confidence that the transaction will not be reversed".
 * <https://docs.minaprotocol.com/mina-protocol/lifecycle-of-a-payment>
 * The node's own finality, k = 290 blocks (its `final` status), comes later.
 */
export const CONFIRMATIONS = 15;

/** Where a block that bought our work stands. */
export type Confirmation = "confirmed" | "pending" | "dropped" | "unknown";

/** Blocks on top of the entry's block: null once it left the best chain,
 * or when the tip isn't known. */
export function confirmations(
  entry: Pick<IncludedEntry, "height" | "status">,
  tipHeight: number | null,
): number | null {
  if (tipHeight === null) return null;
  if (entry.status !== "pending" && entry.status !== "final") return null;
  return Math.max(0, tipHeight - entry.height);
}

export function confirmation(
  entry: Pick<IncludedEntry, "height" | "status">,
  tipHeight: number | null,
): Confirmation {
  if (entry.status === "final") return "confirmed";
  if (entry.status === "pending") {
    const n = confirmations(entry, tipHeight);
    return n !== null && n >= CONFIRMATIONS ? "confirmed" : "pending";
  }
  return entry.status;
}

function addTotals(a: IncludedTotals, e: IncludedEntry): IncludedTotals {
  return {
    blocks: a.blocks + 1,
    bundles: a.bundles + e.bundles,
    zkappBundles: a.zkappBundles + e.zkappBundles,
    fees: a.fees + e.fees,
  };
}

/** Work confirmed (final since the log began, and pending blocks with
 * CONFIRMATIONS on top) and work still pending; `tipHeight` is the log's
 * own, or another recent tip for a node that doesn't send one. */
export function confirmationTotals(
  work: IncludedWork,
  tipHeight: number | null = work.bestTipHeight,
): { confirmed: IncludedTotals; pending: IncludedTotals } {
  let confirmed = work.finalTotals;
  let pending: IncludedTotals = {
    blocks: 0,
    bundles: 0,
    zkappBundles: 0,
    fees: 0,
  };
  for (const e of work.entries) {
    if (e.status !== "pending") continue;
    if (confirmation(e, tipHeight) === "confirmed") {
      confirmed = addTotals(confirmed, e);
    } else {
      pending = addTotals(pending, e);
    }
  }
  return { confirmed, pending };
}

export const confirmationTone: Record<Confirmation, Tone> = {
  confirmed: "ok",
  pending: "info",
  dropped: "warn",
  unknown: "neutral",
};

export const confirmationLabel: Record<Confirmation, string> = {
  confirmed: "Confirmed",
  pending: "Pending",
  dropped: "Dropped",
  unknown: "Unknown",
};

/**
 * The pool's provers split into the node's own keys (`ownKeys`: the
 * included-work log's keys, which the node also counts as its own) and the
 * others: how many there are and their fee range, from the node's totals
 * when it caps the list.
 */
export function marketProvers(
  market: SnarkMarket,
  ownKeys: readonly string[],
): {
  others: SnarkOffer[];
  mine: SnarkOffer[];
  otherCount: number;
  total: number;
  lowestOtherFee: number | null;
  highestOtherFee: number | null;
} {
  const own = new Set(ownKeys);
  const others = market.offers.filter(o => !own.has(o.prover));
  const mine = market.offers.filter(o => own.has(o.prover));
  const otherCount = market.otherProvers ?? others.length;
  return {
    others,
    mine,
    otherCount,
    total: otherCount + mine.length,
    // The cheapest others are always listed.
    lowestOtherFee: others.length
      ? Math.min(...others.map(o => o.minFee))
      : null,
    highestOtherFee:
      market.highestOtherFee ??
      (others.length ? Math.max(...others.map(o => o.maxFee)) : null),
  };
}

/** How this worker's fee compares with the pool's other provers, in a
 * sentence; null when there's nothing to compare. `ownKeys` are the node's
 * other keys (the included-work log's), not counted as others. Fees in
 * nanomina. */
export function feeComparison(
  market: SnarkMarket,
  ourKey: string | null,
  ourFee: number | null,
  ownKeys: readonly string[] = [],
): string | null {
  const p = marketProvers(
    market,
    ourKey === null ? ownKeys : [...ownKeys, ourKey],
  );
  const ours = market.offers.find(o => o.prover === ourKey) ?? null;
  const fee = ours?.minFee ?? ourFee;
  const range = (lo: number, hi: number) =>
    lo === hi
      ? `${formatMina(lo)} MINA`
      : `${formatMina(lo)} to ${formatMina(hi)} MINA`;
  const lo = p.lowestOtherFee;
  const hi = p.highestOtherFee;
  if (p.otherCount === 0 || lo === null || hi === null) {
    if (fee === null) return null;
    return ours
      ? `Only this worker has proofs in the pool, at ${formatMina(fee)} MINA.`
      : "No other prover has proofs in the pool.";
  }
  const n = p.otherCount;
  const ask = n === 1 ? "asks" : "ask";
  if (fee === null) {
    return `${plural(n, "prover")} in the pool ${ask} ${range(lo, hi)} a job.`;
  }
  const where =
    fee < lo
      ? "less than any other prover"
      : fee === lo
        ? "level with the cheapest other prover"
        : fee > hi
          ? "more than any other prover"
          : "inside the others' range";
  return `This worker asks ${formatMina(fee)} MINA a job, ${where}: ${plural(n, "other prover")} ${ask} ${range(lo, hi)}.`;
}
