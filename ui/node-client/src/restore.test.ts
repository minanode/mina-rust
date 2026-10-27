import { expect, test } from "vitest";

import fixture from "./fixtures/stats-sync-devnet.json";
import { parseSyncRestore, restoreHeadline, restoreSummary } from "./restore";

// The Angular app's F-dash3 cases (frontend/tests/restore.test.mjs): the
// load asked for at 02:00:00.000, the replay from 02:00:06.500 to
// 02:00:11.900 UTC.
const ns = (ms: number) => ms * 1_000_000;
const t0 = Date.UTC(2026, 9, 23, 2, 0, 0);
const restored = {
  generation: 7,
  attempt: 1,
  root: "3NKU7ncXikpAZEfEeHuMm1MpuCV5mvcnZCWS5fafCcEvJ93fPabo",
  root_height: 578320,
  tip: "3NKtKuzrhiUG3Hp6KWhDAs1aAQd381Z7yqithKcsmYQdPx8BsT56",
  tip_height: 578610,
  blocks: 290,
  ledgers: 3,
  load_start: ns(t0),
  load_ms: 6_100.5,
  replay_start: ns(t0 + 6_500),
  replay_end: ns(t0 + 11_900) as number | null,
};

test("a committed restore reads as one, with how long it took", () => {
  const r = restoreSummary(restored)!;
  expect(r).toEqual({
    tipHeight: 578610,
    rootHeight: 578320,
    blocks: 290,
    done: true,
    failed: false,
    failure: null,
    totalMs: 11_900,
    endedAt: t0 + 11_900,
  });
  expect(restoreHeadline(r)).toBe(
    "Restored from saved state at #578610 in 12 s",
  );
});

test("while the replay runs, it says so", () => {
  const r = restoreSummary({ ...restored, replay_end: null })!;
  expect(r.done).toBe(false);
  expect(r.totalMs).toBeNull();
  expect(restoreHeadline(r)).toBe(
    "Restoring from saved state: replaying 290 blocks to #578610",
  );
});

test("a replay that failed never reads as replaying", () => {
  const r = restoreSummary({
    ...restored,
    replay_end: null,
    failed: {
      time: ns(t0 + 9_000),
      reason: "block 3NKtKuzr... at 578400: staged ledger hash mismatch",
    },
  })!;
  expect(r).toMatchObject({ failed: true, done: false, totalMs: 9_000 });
  expect(r.failure).toBe(
    "block 3NKtKuzr... at 578400: staged ledger hash mismatch",
  );
  expect(restoreHeadline(r)).toBe(
    "Saved state couldn't be replayed; syncing from the network",
  );
});

test("records from before the failure field, or with fields missing, still read", () => {
  expect(restoreSummary({ ...restored, failed: undefined })!.done).toBe(true);
  const { blocks: _blocks, root_height: _root, ...partial } = restored;
  const r = restoreSummary({ ...partial, replay_end: null })!;
  expect(r).toMatchObject({
    tipHeight: 578610,
    rootHeight: null,
    blocks: null,
  });
  expect(restoreHeadline(r)).toBe(
    "Restoring from saved state: replaying to #578610",
  );
  const { load_start: _start, ...noStart } = restored;
  expect(restoreSummary(noStart)!.totalMs).toBeNull();
});

test("a bootstrap, or an older node, has no restore", () => {
  expect(restoreSummary(undefined)).toBeNull();
  expect(restoreSummary(null)).toBeNull();
  expect(restoreSummary({ tip_height: "x" })).toBeNull();
  expect(parseSyncRestore([])).toBeNull();
  expect(parseSyncRestore(null)).toBeNull();
  expect(parseSyncRestore([{ kind: "Bootstrap", restored: null }])).toBeNull();
  // Before F-dash3 the record had no `restored` at all.
  expect(parseSyncRestore([{ kind: "Catchup", blocks: [] }])).toBeNull();
});

test("the devnet node's catch-up record keeps the restore it started with", () => {
  const r = parseSyncRestore(fixture.records)!;
  expect(r).toMatchObject({
    tipHeight: 578688,
    rootHeight: 578391,
    blocks: 297,
    done: true,
    failed: false,
  });
  // load_start 1792734197505769639 ns to replay_end 1792734206100976975 ns.
  expect(r.totalMs).toBe(8_595);
  expect(restoreHeadline(r)).toBe(
    "Restored from saved state at #578688 in 8.6 s",
  );
});
