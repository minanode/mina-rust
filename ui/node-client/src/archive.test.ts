import { expect, test } from "vitest";

import {
  ARCHIVE_DATABASE_NOT_CONFIGURED,
  archiveBlocksPath,
  archiveDatabaseView,
  archiveFeedView,
  archiveLookup,
  parseArchiveEntries,
  parseArchiveReplay,
  parseArchiveStatus,
} from "./archive";
import { NodeError } from "./errors";
import { formatBytes } from "./format";
import { feedsArchive } from "./react";

const HASH = "3NKbYVr1HwgZ6nRqbnFNLNk6vArKCPu3gr9Fz6v8W9PSdcXFbj3S";
const ref = (height: number) => ({ height, state_hash: HASH });

test("the status parses the node's fields", () => {
  const s = parseArchiveStatus({
    archive_address: "127.0.0.1:3086",
    pending_entries: 2,
    retained_entries: 870,
    retained_bytes: 83_000_000,
    retain_blocks: 870,
    highest_accepted_height: 100,
    oldest_retained: ref(1),
    newest_retained: ref(98),
    last_delivered: ref(98),
    feed_started_at: ref(1),
    gaps_total: 1,
    recent_gaps: [
      { kind: "corrupt", from_height: 5, to_height: 5, state_hash: HASH },
      { kind: "corrupt" },
    ],
    consecutive_failures: 3,
    last_error: "refused",
    failing_since_ms: 1_000,
    stuck_on: ref(99),
  });
  expect(s.newestRetained).toEqual({ height: 98, stateHash: HASH });
  expect(s.recentGaps).toHaveLength(1);
  expect(s.recentGaps[0]).toMatchObject({ kind: "corrupt", fromHeight: 5 });
  expect(s.stuckOn?.height).toBe(99);
  expect(s.failingSinceMs).toBe(1_000);
  // An older node without the new fields still parses.
  const old = parseArchiveStatus({ pending_entries: 0 });
  expect(old.newestRetained).toBeNull();
  expect(old.failingSinceMs).toBeNull();
});

test("the feed's status: failing, not synced, catching up, up to date", () => {
  const base = parseArchiveStatus({
    highest_accepted_height: 100,
    newest_retained: ref(100),
    last_delivered: ref(100),
  });
  const synced = { synced: true, bestTipHeight: 102 };
  expect(archiveFeedView(base, synced)).toMatchObject({
    key: "UpToDate",
    label: "Up to date",
    tone: "ok",
    deliveredHeight: 100,
    behind: 2,
  });
  expect(
    archiveFeedView({ ...base, pendingEntries: 1_234 }, synced),
  ).toMatchObject({ key: "CatchingUp", label: "Catching up (1,234 waiting)" });
  expect(
    archiveFeedView(
      { ...base, pendingEntries: 3, consecutiveFailures: 1 },
      {
        synced: false,
        bestTipHeight: null,
      },
    ),
  ).toMatchObject({ key: "Failing", label: "Delivery failing", behind: null });
  expect(
    archiveFeedView(
      { ...base, pendingEntries: 3 },
      {
        synced: false,
        bestTipHeight: 102,
      },
    ),
  ).toMatchObject({ key: "NotSynced", label: "Node not synced" });
  // An old block's replay makes it the last delivered: the newest retained
  // still says the feed is at its top.
  expect(
    archiveFeedView(
      { ...base, lastDelivered: { height: 40, stateHash: HASH } },
      synced,
    ).key,
  ).toBe("UpToDate");
  // The highest accepted block isn't delivered, or nothing is known yet.
  expect(
    archiveFeedView(
      { ...base, newestRetained: null, lastDelivered: null },
      synced,
    ),
  ).toMatchObject({ key: "NoDelivery", behind: null });
});

test("lookups take a height or a state hash", () => {
  expect(archiveLookup(" 591,571 ")).toEqual({
    kind: "height",
    height: 591_571,
  });
  expect(archiveLookup(HASH)).toEqual({ kind: "hash", stateHash: HASH });
  for (const bad of [
    "",
    "abc",
    "-1",
    "3N",
    "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX",
    "99999999999",
  ]) {
    expect(archiveLookup(bad).kind, bad).toBe("invalid");
  }
  expect(archiveBlocksPath()).toBe("/archive/blocks");
  expect(archiveBlocksPath({ height: 5, limit: 20 })).toBe(
    "/archive/blocks?height=5&limit=20",
  );
  expect(archiveBlocksPath({ stateHash: ` ${HASH} `, state: "retained" })).toBe(
    `/archive/blocks?state_hash=${HASH}&state=retained`,
  );
});

test("entries and replays parse, unknown states left out", () => {
  const list = parseArchiveEntries({
    entries: [
      { height: 3, state_hash: HASH, state: "retained", seq: 3, size: 10 },
      { height: 2, state_hash: HASH, state: "in_flight", seq: 2, size: 10 },
      { height: 1, state_hash: HASH, state: "stored", seq: 1, size: 10 },
    ],
    matching: 3,
    limit: 50,
  });
  expect(list.entries.map(e => e.state)).toEqual(["retained", "in_flight"]);
  expect(list.matching).toBe(3);
  expect(
    parseArchiveReplay({ result: "requeued", requeued: 1, blocks: [ref(3)] }),
  ).toEqual({
    result: "requeued",
    requeued: 1,
    blocks: [{ height: 3, stateHash: HASH }],
  });
  expect(
    parseArchiveReplay({
      result: "already_pending",
      blocks: [],
      in_flight: true,
    }),
  ).toMatchObject({ result: "already_pending", inFlight: true });
  expect(parseArchiveReplay({ result: "requeued", requeued: 870 })).toEqual({
    result: "requeued",
    requeued: 870,
    blocks: [],
  });
});

test("the nav shows the Archive page while the status answers", () => {
  const polled = (data: unknown, error: unknown = null) => ({
    data,
    error,
    updatedAt: data ? 1 : null,
    loading: false,
    refresh: () => {},
  });
  expect(feedsArchive(polled({}))).toBe(true);
  // A node that stops answering keeps its last answer.
  expect(feedsArchive(polled({}, new NodeError("offline", "no answer")))).toBe(
    true,
  );
  // One restarted without the archive, or an older one, hasn't the route.
  expect(
    feedsArchive(polled({}, new NodeError("absent", "no archive", 404))),
  ).toBe(false);
  expect(feedsArchive(polled(null, new NodeError("absent", "", 404)))).toBe(
    false,
  );
  expect(feedsArchive(polled(null))).toBe(false);
});

test("sizes are decimal", () => {
  expect(formatBytes(512)).toBe("512 B");
  expect(formatBytes(98_234_567)).toBe("98 MB");
  expect(formatBytes(1_400_000_000)).toBe("1.4 GB");
  expect(formatBytes(null)).toBe("–");
});

const DATABASE = {
  configured: true,
  metrics_url: "http://127.0.0.1:3087/metrics",
  verdict: "incomplete",
  last_attempt_at_ms: 2_000,
  last_success_at_ms: 2_000,
  last_error: null,
  max_block_height: 98,
  missing_blocks: 1,
  unparented_blocks: 1,
  delivered_height: 100,
  stored_tip_lag: 2,
  expected_stored_height: 97,
  behind_after_ms: 360_000,
  poll_interval_ms: 60_000,
};

test("the database check parses; an older node has none", () => {
  const s = parseArchiveStatus({
    highest_delivered: ref(100),
    database: DATABASE,
  });
  expect(s.highestDelivered).toEqual({ height: 100, stateHash: HASH });
  expect(s.database).toEqual({
    configured: true,
    metricsUrl: "http://127.0.0.1:3087/metrics",
    verdict: "incomplete",
    lastAttemptMs: 2_000,
    lastSuccessMs: 2_000,
    lastError: null,
    maxBlockHeight: 98,
    missingBlocks: 1,
    unparentedBlocks: 1,
    deliveredHeight: 100,
    storedTipLag: 2,
    expectedStoredHeight: 97,
    behindAfterMs: 360_000,
    pollIntervalMs: 60_000,
  });
  expect(parseArchiveStatus({}).database).toEqual(
    ARCHIVE_DATABASE_NOT_CONFIGURED,
  );
  // A verdict this dashboard doesn't know isn't taken for a good one.
  expect(
    parseArchiveStatus({ database: { ...DATABASE, verdict: "new_thing" } })
      .database.verdict,
  ).toBe("not_checked");
});

test("the database verdict in words", () => {
  const db = parseArchiveStatus({ database: DATABASE }).database;
  expect(archiveDatabaseView({ ...db, verdict: "complete" })).toEqual({
    label: "Complete",
    tone: "ok",
  });
  expect(archiveDatabaseView(db)).toEqual({
    label: "Incomplete",
    tone: "error",
  });
  expect(archiveDatabaseView({ ...db, verdict: "behind" }).label).toBe(
    "Behind",
  );
  expect(archiveDatabaseView({ ...db, verdict: "unavailable" }).label).toBe(
    "Unavailable",
  );
  expect(archiveDatabaseView({ ...db, verdict: "not_checked" }).label).toBe(
    "Checking",
  );
  expect(archiveDatabaseView(ARCHIVE_DATABASE_NOT_CONFIGURED).label).toBe(
    "Not checked",
  );
});

test("the feed's status: failing > not synced > database > catching up", () => {
  const base = parseArchiveStatus({
    highest_accepted_height: 100,
    newest_retained: ref(100),
    last_delivered: ref(100),
    database: DATABASE,
  });
  const synced = { synced: true, bestTipHeight: 100 };
  const incomplete = base;
  const behind = {
    ...base,
    database: { ...base.database, verdict: "behind" as const },
  };
  expect(archiveFeedView(incomplete, synced)).toMatchObject({
    key: "DatabaseIncomplete",
    label: "Database incomplete",
    tone: "error",
  });
  expect(archiveFeedView(behind, synced)).toMatchObject({
    key: "DatabaseBehind",
    label: "Database behind",
    tone: "warn",
  });
  // Above catching up.
  expect(
    archiveFeedView({ ...incomplete, pendingEntries: 5 }, synced).key,
  ).toBe("DatabaseIncomplete");
  expect(archiveFeedView({ ...behind, pendingEntries: 5 }, synced).key).toBe(
    "DatabaseBehind",
  );
  // Below failing and not synced.
  expect(
    archiveFeedView({ ...incomplete, consecutiveFailures: 2 }, synced).key,
  ).toBe("Failing");
  expect(
    archiveFeedView(incomplete, { synced: false, bestTipHeight: 100 }).key,
  ).toBe("NotSynced");
  // Complete, unavailable and not checked leave the feed's own status.
  for (const verdict of ["complete", "unavailable", "not_checked"] as const) {
    const s = { ...base, database: { ...base.database, verdict } };
    expect(archiveFeedView(s, synced).key).toBe("UpToDate");
    expect(archiveFeedView({ ...s, pendingEntries: 5 }, synced).key).toBe(
      "CatchingUp",
    );
  }
});
