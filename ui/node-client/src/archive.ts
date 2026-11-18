/**
 * The archive feed of a node that sends its blocks to an OCaml archive
 * process (`--archive-archiver-process`): `GET /archive/status`,
 * `GET /archive/blocks` and `POST /archive/resend`. A node that doesn't feed
 * an archive answers 404 on all three.
 *
 * "Delivered" means the archive process acknowledged the diff. The archive
 * acknowledges before its database commit, so the node can't see whether a
 * block is stored. With `MINA_ARCHIVE_METRICS_ADDRESS` the status's
 * `database` section has the archive's own counts of its database, read from
 * its `--metrics-port`.
 */

import { arr, num, obj, str } from "./json";

/** The header a replay request must carry. */
export const ARCHIVE_CONTROL_HEADER = "x-mina-archive-control";

export interface ArchiveBlockRef {
  height: number;
  stateHash: string;
}

export type ArchiveGapKind = "unseen_parent" | "dropped_full" | "corrupt";

/** History this node won't deliver: blocks it never saw, dropped, or found
 * corrupt. */
export interface ArchiveGap {
  kind: ArchiveGapKind | string;
  fromHeight: number;
  toHeight: number;
  /** The missing block, when known (dropped and corrupt entries). */
  stateHash: string | null;
  /** The missing parent, for `unseen_parent`. */
  parentHash: string | null;
  /** The block that revealed an `unseen_parent` gap. */
  firstBlockAfter: ArchiveBlockRef | null;
  detectedMs: number | null;
}

/** The archive database check's verdict. */
export type ArchiveDatabaseVerdict =
  | "complete"
  | "incomplete"
  | "behind"
  | "not_checked"
  | "unavailable";

const DATABASE_VERDICTS: readonly string[] = [
  "complete",
  "incomplete",
  "behind",
  "not_checked",
  "unavailable",
];

/**
 * The archive's own counts of its database, from its Prometheus gauges:
 * `Coda_Archive_max_block_height`, `Coda_Archive_missing_blocks` (heights
 * with no block in the archive's recent window, 2,000 heights by default)
 * and `Coda_Archive_unparented_blocks`. The archive refreshes them every two
 * slots.
 */
export interface ArchiveDatabase {
  /** Whether the node has `MINA_ARCHIVE_METRICS_ADDRESS`. */
  configured: boolean;
  metricsUrl: string | null;
  verdict: ArchiveDatabaseVerdict;
  lastAttemptMs: number | null;
  /** When the values were read. */
  lastSuccessMs: number | null;
  lastError: string | null;
  maxBlockHeight: number | null;
  missingBlocks: number | null;
  unparentedBlocks: number | null;
  /** The highest block delivered, when the values were read. */
  deliveredHeight: number | null;
  /** deliveredHeight - maxBlockHeight; negative when the database has more. */
  storedTipLag: number | null;
  /** The stored tip must reach this: the highest block delivered
   * behindAfterMs before the values were read. */
  expectedStoredHeight: number | null;
  behindAfterMs: number;
  pollIntervalMs: number;
}

export const ARCHIVE_DATABASE_NOT_CONFIGURED: ArchiveDatabase = {
  configured: false,
  metricsUrl: null,
  verdict: "not_checked",
  lastAttemptMs: null,
  lastSuccessMs: null,
  lastError: null,
  maxBlockHeight: null,
  missingBlocks: null,
  unparentedBlocks: null,
  deliveredHeight: null,
  storedTipLag: null,
  expectedStoredHeight: null,
  behindAfterMs: 0,
  pollIntervalMs: 0,
};

export interface ArchiveStatus {
  archiveAddress: string | null;
  pendingEntries: number;
  pendingBytes: number;
  retainedEntries: number;
  retainedBytes: number;
  retainBlocks: number;
  highestAcceptedHeight: number;
  oldestRetained: ArchiveBlockRef | null;
  newestRetained: ArchiveBlockRef | null;
  oldestPending: ArchiveBlockRef | null;
  newestPending: ArchiveBlockRef | null;
  feedStartedAt: ArchiveBlockRef | null;
  /** Diffs delivered since the outbox was opened. */
  deliveredCount: number;
  gapsTotal: number;
  /** The most recent gaps, oldest first. */
  recentGaps: ArchiveGap[];
  lastDelivered: ArchiveBlockRef | null;
  consecutiveFailures: number;
  lastError: string | null;
  lastAttemptMs: number | null;
  lastSuccessMs: number | null;
  failingSinceMs: number | null;
  /** The entry being retried while delivery fails. */
  stuckOn: ArchiveBlockRef | null;
  /** The highest block acknowledged since the node started; a replay of
   * older blocks doesn't lower it. */
  highestDelivered: ArchiveBlockRef | null;
  database: ArchiveDatabase;
}

export type ArchiveEntryState = "pending" | "in_flight" | "retained";

export interface ArchiveEntry {
  height: number;
  stateHash: string;
  state: ArchiveEntryState;
  seq: number;
  size: number;
}

export interface ArchiveEntries {
  entries: ArchiveEntry[];
  /** Entries that match, before the limit. */
  matching: number;
  limit: number;
  oldestRetained: ArchiveBlockRef | null;
  newestRetained: ArchiveBlockRef | null;
  retainBlocks: number;
  highestAcceptedHeight: number;
}

export interface ArchiveQuery {
  height?: number;
  stateHash?: string;
  state?: "pending" | "retained";
  limit?: number;
}

/** What a replay did. `notRetained` comes back as a 404 NodeError. */
export type ArchiveReplay =
  | { result: "requeued"; requeued: number; blocks: ArchiveBlockRef[] }
  | {
      result: "already_pending";
      requeued: 0;
      blocks: ArchiveBlockRef[];
      inFlight: boolean;
    };

function blockRef(value: unknown): ArchiveBlockRef | null {
  const b = obj(value);
  const height = num(b?.height);
  const stateHash = str(b?.state_hash);
  return height !== null && stateHash ? { height, stateHash } : null;
}

const count = (value: unknown) => num(value) ?? 0;

function parseGap(value: unknown): ArchiveGap | null {
  const g = obj(value);
  const from = num(g?.from_height);
  const to = num(g?.to_height);
  if (!g || from === null || to === null) return null;
  return {
    kind: str(g.kind) ?? "unknown",
    fromHeight: from,
    toHeight: to,
    stateHash: str(g.state_hash),
    parentHash: str(g.parent_hash),
    firstBlockAfter: blockRef(g.first_block_after),
    detectedMs: num(g.detected_at_ms),
  };
}

/** `database` in `/archive/status`; a node without the check (or older
 * than it) is not configured. */
export function parseArchiveDatabase(raw: unknown): ArchiveDatabase {
  const d = obj(raw);
  if (!d) return ARCHIVE_DATABASE_NOT_CONFIGURED;
  const verdict = str(d.verdict);
  return {
    configured: d.configured === true,
    metricsUrl: str(d.metrics_url),
    verdict:
      verdict && DATABASE_VERDICTS.includes(verdict)
        ? (verdict as ArchiveDatabaseVerdict)
        : "not_checked",
    lastAttemptMs: num(d.last_attempt_at_ms),
    lastSuccessMs: num(d.last_success_at_ms),
    lastError: str(d.last_error),
    maxBlockHeight: num(d.max_block_height),
    missingBlocks: num(d.missing_blocks),
    unparentedBlocks: num(d.unparented_blocks),
    deliveredHeight: num(d.delivered_height),
    storedTipLag: num(d.stored_tip_lag),
    expectedStoredHeight: num(d.expected_stored_height),
    behindAfterMs: count(d.behind_after_ms),
    pollIntervalMs: count(d.poll_interval_ms),
  };
}

export function parseArchiveStatus(raw: unknown): ArchiveStatus {
  const s = obj(raw) ?? {};
  return {
    archiveAddress: str(s.archive_address),
    pendingEntries: count(s.pending_entries),
    pendingBytes: count(s.pending_bytes),
    retainedEntries: count(s.retained_entries),
    retainedBytes: count(s.retained_bytes),
    retainBlocks: count(s.retain_blocks),
    highestAcceptedHeight: count(s.highest_accepted_height),
    oldestRetained: blockRef(s.oldest_retained),
    newestRetained: blockRef(s.newest_retained),
    oldestPending: blockRef(s.oldest_pending),
    newestPending: blockRef(s.newest_pending),
    feedStartedAt: blockRef(s.feed_started_at),
    deliveredCount: count(s.delivered),
    gapsTotal: count(s.gaps_total),
    recentGaps: arr(s.recent_gaps)
      .map(parseGap)
      .filter((g): g is ArchiveGap => g !== null),
    lastDelivered: blockRef(s.last_delivered),
    consecutiveFailures: count(s.consecutive_failures),
    lastError: str(s.last_error),
    lastAttemptMs: num(s.last_attempt_at_ms),
    lastSuccessMs: num(s.last_success_at_ms),
    failingSinceMs: num(s.failing_since_ms),
    stuckOn: blockRef(s.stuck_on),
    highestDelivered: blockRef(s.highest_delivered),
    database: parseArchiveDatabase(s.database),
  };
}

const ENTRY_STATES: readonly string[] = ["pending", "in_flight", "retained"];

export function parseArchiveEntries(raw: unknown): ArchiveEntries {
  const l = obj(raw) ?? {};
  return {
    entries: arr(l.entries).flatMap(value => {
      const e = obj(value);
      const height = num(e?.height);
      const stateHash = str(e?.state_hash);
      const state = str(e?.state);
      if (height === null || !stateHash || !state) return [];
      if (!ENTRY_STATES.includes(state)) return [];
      return [
        {
          height,
          stateHash,
          state: state as ArchiveEntryState,
          seq: count(e?.seq),
          size: count(e?.size),
        },
      ];
    }),
    matching: count(l.matching),
    limit: count(l.limit),
    oldestRetained: blockRef(l.oldest_retained),
    newestRetained: blockRef(l.newest_retained),
    retainBlocks: count(l.retain_blocks),
    highestAcceptedHeight: count(l.highest_accepted_height),
  };
}

export function parseArchiveReplay(raw: unknown): ArchiveReplay {
  const r = obj(raw) ?? {};
  const blocks = arr(r.blocks)
    .map(blockRef)
    .filter((b): b is ArchiveBlockRef => b !== null);
  if (str(r.result) === "already_pending") {
    return {
      result: "already_pending",
      requeued: 0,
      blocks,
      inFlight: r.in_flight === true,
    };
  }
  return { result: "requeued", requeued: count(r.requeued), blocks };
}

/** The query string of `GET /archive/blocks`. */
export function archiveBlocksPath(query: ArchiveQuery = {}): string {
  const params = new URLSearchParams();
  if (query.height !== undefined)
    params.set("height", String(Math.max(0, Math.floor(query.height))));
  if (query.stateHash?.trim()) params.set("state_hash", query.stateHash.trim());
  if (query.state) params.set("state", query.state);
  if (query.limit !== undefined)
    params.set("limit", String(Math.max(0, Math.floor(query.limit))));
  const search = params.toString();
  return `/archive/blocks${search ? `?${search}` : ""}`;
}

/** What the operator typed in the replay box: a height or a state hash. */
export type ArchiveLookup =
  | { kind: "height"; height: number }
  | { kind: "hash"; stateHash: string }
  | { kind: "invalid"; message: string };

export function archiveLookup(input: string): ArchiveLookup {
  const text = input.trim().replace(/,/g, "");
  if (/^\d+$/.test(text)) {
    const height = Number(text);
    if (Number.isSafeInteger(height) && height <= 0xffff_ffff) {
      return { kind: "height", height };
    }
  }
  const hash = input.trim();
  if (/^3N[1-9A-HJ-NP-Za-km-z]{48,60}$/.test(hash)) {
    return { kind: "hash", stateHash: hash };
  }
  return {
    kind: "invalid",
    message: "Enter a block height or a state hash (3N…).",
  };
}

/** The feed at a glance. */
export type ArchiveFeedKey =
  | "UpToDate"
  | "CatchingUp"
  | "Failing"
  | "NotSynced"
  | "DatabaseIncomplete"
  | "DatabaseBehind"
  | "NoDelivery";

export interface ArchiveFeedView {
  key: ArchiveFeedKey;
  label: string;
  tone: "ok" | "info" | "warn" | "error" | "neutral";
  /** The height of the newest block the archive acknowledged. */
  deliveredHeight: number | null;
  /** Blocks between the node's best tip and that block; null when either
   * isn't known. */
  behind: number | null;
}

/**
 * Up to date: nothing waiting, no failures, and the newest delivered block
 * is the highest the node accepted. Failing comes first, then a node that
 * isn't synced, whose feed can't be up to date with the network, then the
 * database check when it found a problem (incomplete or behind), then
 * catching up.
 */
export function archiveFeedView(
  status: ArchiveStatus,
  node: { synced: boolean; bestTipHeight: number | null },
): ArchiveFeedView {
  const delivered = [
    status.newestRetained,
    status.lastDelivered,
    status.highestDelivered,
  ]
    .map(b => b?.height ?? null)
    .filter((h): h is number => h !== null);
  const deliveredHeight = delivered.length > 0 ? Math.max(...delivered) : null;
  const behind =
    node.bestTipHeight !== null && deliveredHeight !== null
      ? Math.max(0, node.bestTipHeight - deliveredHeight)
      : null;
  const base = { deliveredHeight, behind };
  if (status.consecutiveFailures > 0) {
    return {
      ...base,
      key: "Failing",
      label: "Delivery failing",
      tone: "error",
    };
  }
  if (!node.synced) {
    return {
      ...base,
      key: "NotSynced",
      label: "Node not synced",
      tone: "warn",
    };
  }
  if (status.database.verdict === "incomplete") {
    return {
      ...base,
      key: "DatabaseIncomplete",
      label: "Database incomplete",
      tone: "error",
    };
  }
  if (status.database.verdict === "behind") {
    return {
      ...base,
      key: "DatabaseBehind",
      label: "Database behind",
      tone: "warn",
    };
  }
  if (status.pendingEntries > 0) {
    return {
      ...base,
      key: "CatchingUp",
      label: `Catching up (${status.pendingEntries.toLocaleString("en-US")} waiting)`,
      tone: "info",
    };
  }
  if (
    deliveredHeight !== null &&
    status.highestAcceptedHeight > 0 &&
    deliveredHeight >= status.highestAcceptedHeight
  ) {
    return { ...base, key: "UpToDate", label: "Up to date", tone: "ok" };
  }
  return {
    ...base,
    key: "NoDelivery",
    label: "Nothing delivered yet",
    tone: "neutral",
  };
}

export interface ArchiveDatabaseView {
  label: string;
  tone: "ok" | "info" | "warn" | "error" | "neutral";
}

/** The database check's verdict in words. */
export function archiveDatabaseView(db: ArchiveDatabase): ArchiveDatabaseView {
  switch (db.verdict) {
    case "complete":
      return { label: "Complete", tone: "ok" };
    case "incomplete":
      return { label: "Incomplete", tone: "error" };
    case "behind":
      return { label: "Behind", tone: "warn" };
    case "unavailable":
      return { label: "Unavailable", tone: "warn" };
    case "not_checked":
      return db.configured
        ? { label: "Checking", tone: "neutral" }
        : { label: "Not checked", tone: "neutral" };
  }
}

/** A gap's kind in words. */
export function archiveGapLabel(kind: string): string {
  switch (kind) {
    case "unseen_parent":
      return "Never seen";
    case "dropped_full":
      return "Dropped, outbox full";
    case "corrupt":
      return "Corrupt entry";
    default:
      return kind;
  }
}

/** An entry's state in words. */
export function archiveEntryLabel(state: ArchiveEntryState): string {
  switch (state) {
    case "pending":
      return "Waiting";
    case "in_flight":
      return "Sending";
    case "retained":
      return "Delivered";
  }
}
