/**
 * A restart from saved state (the frontier store), as `/stats/sync` reports
 * it since F-dash3: a record of kind `Restored` with a `restored` summary,
 * kept on the catch-ups after it. A fresh bootstrap has none. Ported from the
 * Angular app's restore helper (frontend/src/app/shared/helpers).
 */

import { formatDuration, formatHeight, plural } from "./format";
import { arr, nanosToMs, num, obj, str } from "./json";

export interface RestoreSummary {
  /** The stored tip the node restarted from. */
  tipHeight: number;
  rootHeight: number | null;
  /** Blocks replayed after the root. */
  blocks: number | null;
  /** The replay committed. */
  done: boolean;
  /** The replay failed: the node syncs from the network instead. */
  failed: boolean;
  failure: string | null;
  /** From asking for the load to the replay's commit or failure. */
  totalMs: number | null;
  /** When the replay committed or failed, ms since the epoch. */
  endedAt: number | null;
}

/** The node's `SyncRestored`, or null if the value isn't one. */
export function restoreSummary(value: unknown): RestoreSummary | null {
  const r = obj(value);
  const tipHeight = num(r?.tip_height);
  if (!r || tipHeight === null) return null;
  const failedRecord = obj(r.failed);
  const failedAt = failedRecord ? nanosToMs(failedRecord.time) : null;
  const end = failedRecord ? failedAt : nanosToMs(r.replay_end);
  const start = nanosToMs(r.load_start);
  return {
    tipHeight,
    rootHeight: num(r.root_height),
    blocks: num(r.blocks),
    done: !failedRecord && end !== null,
    failed: failedRecord !== null,
    failure: failedRecord ? str(failedRecord.reason) : null,
    totalMs: end !== null && start !== null ? end - start : null,
    endedAt: end,
  };
}

/** The restore of this run, from `/stats/sync?limit=1`: the newest record's
 * `restored`. Null after a fresh bootstrap, and from nodes before F-dash3. */
export function parseSyncRestore(raw: unknown): RestoreSummary | null {
  const latest = obj(arr(raw)[0]);
  return latest ? restoreSummary(latest.restored) : null;
}

/** "Restored from saved state at #578605 in 12 s"; while the replay runs,
 * "Restoring from saved state: replaying 290 blocks to #578605". */
export function restoreHeadline(r: RestoreSummary): string {
  if (r.failed) {
    return "Saved state couldn't be replayed; syncing from the network";
  }
  if (r.done && r.totalMs !== null) {
    return `Restored from saved state at ${formatHeight(r.tipHeight)} in ${formatDuration(r.totalMs)}`;
  }
  const blocks = r.blocks !== null ? `${plural(r.blocks, "block")} ` : "";
  return `Restoring from saved state: replaying ${blocks}to ${formatHeight(r.tipHeight)}`;
}
