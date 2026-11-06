/**
 * The boot screen's sync graphic, from the node's `/stats/sync/progress`:
 * one row a ledger (staking epoch, next epoch, the root's snarked and then
 * staged ledger), and a strip of the frontier's blocks that fills as they
 * are fetched and applied. The node reports a ledger's counts only while it
 * fetches it, so the boot keeps the last ones it saw for the rows it has
 * finished.
 */

import {
  formatCount,
  type LedgerProgress,
  type SyncProgress,
} from "@minanode/node-client";

export type LedgerKey =
  | "stakingEpoch"
  | "nextEpoch"
  | "rootSnarked"
  | "rootStaged";

export const LEDGERS: { key: LedgerKey; label: string }[] = [
  { key: "stakingEpoch", label: "Staking epoch ledger" },
  { key: "nextEpoch", label: "Next epoch ledger" },
  { key: "rootSnarked", label: "Root snarked ledger" },
  { key: "rootStaged", label: "Root staged ledger" },
];

/** The last counts seen for each ledger while it synced. */
export type LedgerMemory = Partial<Record<LedgerKey, LedgerProgress>>;

export function rememberLedgers(
  memory: LedgerMemory,
  progress: SyncProgress,
): LedgerMemory {
  let next = memory;
  for (const { key } of LEDGERS) {
    const p = progress[key];
    if (
      p.status === "syncing" &&
      (p.total !== null || p.accountsTotal !== null)
    ) {
      next = { ...next, [key]: p };
    }
  }
  return next;
}

export interface LedgerRow {
  key: LedgerKey;
  label: string;
  status: LedgerProgress["status"];
  /** 0 to 100; null when unknown. */
  percent: number | null;
  text: string;
}

function percent(done: number | null, total: number | null): number | null {
  if (done === null || total === null || total <= 0) return null;
  return Math.max(0, Math.min(100, Math.floor((done / total) * 100)));
}

function counts(key: LedgerKey, p: LedgerProgress): string | null {
  if (key === "rootStaged") {
    if (p.fetched === null || p.total === null) return null;
    // 0 of 1: the node hasn't heard how many parts there are.
    if (p.fetched === 0 && p.total <= 1) return null;
    return `${formatCount(p.fetched)} of ${formatCount(p.total)} parts`;
  }
  if (p.accountsFetched !== null && p.accountsTotal !== null) {
    return `${formatCount(p.accountsFetched)} of ${formatCount(p.accountsTotal)} accounts`;
  }
  return null;
}

export function ledgerRows(
  progress: SyncProgress,
  memory: LedgerMemory = {},
): LedgerRow[] {
  return LEDGERS.map(({ key, label }) => {
    const p = progress[key];
    if (p.status === "done") {
      const last = memory[key];
      const total =
        key === "rootStaged" ? last?.total : (last?.accountsTotal ?? null);
      return {
        key,
        label,
        status: "done",
        percent: 100,
        text:
          total !== null &&
          total !== undefined &&
          !(key === "rootStaged" && total <= 1)
            ? `${formatCount(total)} ${key === "rootStaged" ? "parts" : "accounts"}`
            : "Done",
      };
    }
    if (p.status === "waiting") {
      return { key, label, status: "waiting", percent: 0, text: "Waiting" };
    }
    const bar =
      key === "rootStaged"
        ? percent(p.fetched, p.total)
        : (percent(p.fetched, p.total) ??
          percent(p.accountsFetched, p.accountsTotal));
    return {
      key,
      label,
      status: "syncing",
      percent: bar,
      text:
        counts(key, p) ??
        (key === "rootStaged"
          ? "Asking a peer for the parts"
          : "Asking peers for its size"),
    };
  });
}

export type CellState =
  | "missing"
  | "fetching"
  | "fetched"
  | "applying"
  | "applied"
  | "failed";

const CELL: Record<string, CellState> = {
  ".": "missing",
  f: "fetching",
  F: "fetched",
  a: "applying",
  A: "applied",
  x: "failed",
};

export interface BlockStrip {
  /** One a block, the root first. */
  cells: CellState[];
  total: number;
  fetched: number;
  applied: number;
  failed: number;
  /** Not applied yet. */
  toGo: number;
  /** The node's k, for the frontier's size. */
  k: number | null;
  /** The node is still on its ledgers: the strip is the frontier to come. */
  waiting: boolean;
}

/** The frontier strip. Before the blocks stage, k + 1 empty cells: the
 * frontier the sync will fill. Null when the node gives neither. */
export function blockStrip(progress: SyncProgress): BlockStrip | null {
  const b = progress.blocks;
  if (b) {
    const cells = [...b.cells].map(c => CELL[c] ?? "missing");
    while (cells.length < b.total) cells.push("missing");
    return {
      cells,
      total: b.total,
      fetched: b.fetched,
      applied: b.applied,
      failed: b.failed,
      toGo: Math.max(0, b.total - b.applied),
      k: progress.k,
      waiting: false,
    };
  }
  if (progress.k === null || progress.stage === "Synced") return null;
  const total = progress.k + 1;
  return {
    cells: Array.from({ length: total }, () => "missing"),
    total,
    fetched: 0,
    applied: 0,
    failed: 0,
    toGo: total,
    k: progress.k,
    waiting: true,
  };
}

/** The strip in a sentence, for its label and the step's detail. */
export function stripSentence(strip: BlockStrip): string {
  if (strip.waiting) {
    return `${formatCount(strip.total)} blocks to fetch and apply once the ledgers are in.`;
  }
  if (strip.toGo === 0) {
    return `All ${formatCount(strip.total)} blocks applied.`;
  }
  return `${formatCount(strip.toGo)} of ${formatCount(strip.total)} to go: ${formatCount(strip.fetched)} fetched, ${formatCount(strip.applied)} applied${strip.failed ? `, ${formatCount(strip.failed)} failed` : ""}.`;
}

/** The ledger step's detail line: how far through the ledgers it is. The
 * rows under it name each ledger. */
export function ledgerStepSentence(progress: SyncProgress): string {
  const rows = LEDGERS.map(({ key, label }) => ({
    label,
    status: progress[key].status,
  }));
  const done = rows.filter(r => r.status === "done").length;
  const current = rows.find(r => r.status === "syncing");
  return current
    ? `Ledger ${done + 1} of ${rows.length}.`
    : `${done} of ${rows.length} ledgers done.`;
}
