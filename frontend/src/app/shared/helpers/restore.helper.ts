/**
 * A restart from saved state (the frontier store), as the node's sync record
 * (`/stats/sync`) reports it since F-dash3: a record of kind `Restored` with a
 * `restored` summary, kept on the catch-ups after it, and its ledgers marked
 * `from_store`. A fresh bootstrap has none of these. Pure functions, so that
 * they can be tested without Angular (frontend/tests/restore.test.mjs).
 */

/** The node's `SyncRestored`; times in nanoseconds since the epoch. */
export interface SyncRestoredResponse {
  generation: number;
  attempt: number;
  root: string;
  root_height: number;
  tip: string;
  tip_height: number;
  blocks: number;
  ledgers: number;
  load_start: number;
  load_ms: number;
  replay_start: number;
  replay_end: number | null;
  /** The replay failed (absent from nodes before the F-dash3 fix round). */
  failed?: { time: number; reason: string } | null;
}

export interface RestoreSummary {
  /** The stored tip the node restarted from. */
  tipHeight: number;
  rootHeight: number;
  /** Blocks replayed after the root. */
  blocks: number;
  /** The replay committed. */
  done: boolean;
  /**
   * The replay failed: the saved state is dropped and the node syncs from the
   * network. `failure` says why.
   */
  failed: boolean;
  failure: string | null;
  /** From asking for the load to the replay's commit or failure. */
  totalMs: number | null;
  /** When the replay committed or failed, ms since the epoch. */
  endedAt: number | null;
}

const nsToMs = (ns: number): number => Math.floor(ns / 1_000_000);

export function restoreSummary(
  restored: SyncRestoredResponse | null | undefined,
): RestoreSummary | null {
  if (!restored || !Number.isFinite(restored.tip_height)) {
    return null;
  }
  const failed = restored.failed ?? null;
  const end = failed
    ? nsToMs(failed.time)
    : restored.replay_end
      ? nsToMs(restored.replay_end)
      : null;
  return {
    tipHeight: restored.tip_height,
    rootHeight: restored.root_height,
    blocks: restored.blocks,
    done: !failed && end !== null,
    failed: failed !== null,
    failure: failed?.reason ?? null,
    totalMs: end === null ? null : end - nsToMs(restored.load_start),
    endedAt: end,
  };
}

/** "0.8 s", "12 s", "1 min 5 s": how long a restore took. */
export function restoreDuration(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  if (s < 10) {
    return `${s.toFixed(1)} s`;
  }
  if (s < 60) {
    return `${Math.round(s)} s`;
  }
  const whole = Math.round(s);
  const m = Math.floor(whole / 60);
  return whole % 60 ? `${m} min ${whole % 60} s` : `${m} min`;
}

/**
 * The dashboard header: "Restored from saved state at #578605 in 12 s", while
 * the replay runs "Restoring from saved state: replaying 290 blocks to
 * #578605", and if it failed "Saved state couldn't be replayed; syncing from
 * the network".
 */
export function restoreHeadline(r: RestoreSummary): string {
  if (r.failed) {
    return "Saved state couldn't be replayed; syncing from the network";
  }
  if (r.done && r.totalMs !== null) {
    return `Restored from saved state at #${r.tipHeight} in ${restoreDuration(r.totalMs)}`;
  }
  const blocks = `${r.blocks} block${r.blocks === 1 ? '' : 's'}`;
  return `Restoring from saved state: replaying ${blocks} to #${r.tipHeight}`;
}

/** The Ledgers card's note, next to its title: short, the card is narrow. */
export function restoreLedgersNote(r: RestoreSummary): string {
  if (r.failed) {
    return 'saved state not used';
  }
  if (r.done && r.totalMs !== null) {
    return `restored at #${r.tipHeight} in ${restoreDuration(r.totalMs)}`;
  }
  return 'loaded from saved state';
}
