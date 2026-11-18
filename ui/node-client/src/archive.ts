/**
 * The archive feed of a node that sends its blocks to an OCaml archive
 * process (`--archive-archiver-process`): `GET /archive/status`,
 * `GET /archive/blocks` and `POST /archive/resend`. A node that doesn't feed
 * an archive answers 404 on all three.
 *
 * "Delivered" means the archive process acknowledged the diff. The archive
 * acknowledges before its database commit, so the node can't see whether a
 * block is stored.
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
 * isn't synced, whose feed can't be up to date with the network.
 */
export function archiveFeedView(
  status: ArchiveStatus,
  node: { synced: boolean; bestTipHeight: number | null },
): ArchiveFeedView {
  const delivered = [status.newestRetained, status.lastDelivered]
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
