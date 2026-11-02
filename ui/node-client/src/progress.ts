/**
 * How far a bootstrap has got: the ledgers it fetches first, from
 * `GET /state/message-progress`, then the blocks between the root and the
 * tip, from the newest record of `GET /stats/sync?limit=1`. A web node
 * answers both the same, from `state().message_progress()` and
 * `stats().sync(1)`.
 */

import { arr, num, obj, str } from "./json";

export interface Fraction {
  done: number;
  total: number;
}

export interface LedgerSync {
  /** The staking epoch ledger's accounts, fetched of estimated. */
  stakingEpoch: Fraction | null;
  nextEpoch: Fraction | null;
  /** The root's snarked ledger. */
  root: Fraction | null;
  /** The root's staged ledger parts, once the snarked one is in. */
  rootStaged: Fraction | null;
}

function fraction(
  value: unknown,
  doneField: string,
  totalField: string,
): Fraction | null {
  const v = obj(value);
  const done = num(v?.[doneField]);
  const total = num(v?.[totalField]);
  if (done === null || total === null) return null;
  return { done, total };
}

export function parseLedgerSync(raw: unknown): LedgerSync {
  const r = obj(raw) ?? {};
  const root = obj(r.root_ledger_sync);
  return {
    stakingEpoch: fraction(r.staking_ledger_sync, "fetched", "estimation"),
    nextEpoch: fraction(r.next_epoch_ledger_sync, "fetched", "estimation"),
    root: fraction(root, "fetched", "estimation"),
    rootStaged: fraction(root?.staged, "fetched", "total"),
  };
}

/** The ledgers' accounts fetched of the estimate, summed; null before any
 * ledger sync started. */
export function ledgerTotal(sync: LedgerSync): Fraction | null {
  const parts = [sync.stakingEpoch, sync.nextEpoch, sync.root].filter(
    (p): p is Fraction => p !== null,
  );
  if (parts.length === 0) return null;
  return parts.reduce(
    (sum, p) => ({
      done: sum.done + Math.min(p.done, p.total),
      total: sum.total + p.total,
    }),
    { done: 0, total: 0 },
  );
}

export interface SyncBlocks {
  /** Bootstrap, Catchup or Restored. */
  kind: string | null;
  /** Blocks the sync needs, root to tip. */
  total: number;
  /** Of those, fetched or further along. */
  fetched: number;
  applied: number;
}

/** The blocks of the newest sync record; null without one. */
export function parseSyncBlocks(raw: unknown): SyncBlocks | null {
  const latest = obj(arr(raw)[0]);
  if (!latest) return null;
  const blocks = arr(latest.blocks).map(b => str(obj(b)?.status));
  const fetchedOrLater = new Set(["Fetched", "Applying", "Applied"]);
  return {
    kind: str(latest.kind),
    total: blocks.length,
    fetched: blocks.filter(s => s !== null && fetchedOrLater.has(s)).length,
    applied: blocks.filter(s => s === "Applied").length,
  };
}
