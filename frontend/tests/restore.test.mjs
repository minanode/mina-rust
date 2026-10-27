import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  restoreDuration,
  restoreHeadline,
  restoreLedgersNote,
  restoreSummary,
} from '../src/app/shared/helpers/restore.helper.ts';

// F-dash3: a restart from saved state, as the node's sync record reports it
// (`SyncRestored` in crates/node/src/stats/stats_sync.rs): the load asked for
// at 02:00:00.000, the replay from 02:00:06.500 to 02:00:11.900 UTC.
const ns = ms => ms * 1_000_000;
const t0 = Date.UTC(2026, 9, 23, 2, 0, 0);
const restored = {
  generation: 7,
  attempt: 1,
  root: '3NKU7ncXikpAZEfEeHuMm1MpuCV5mvcnZCWS5fafCcEvJ93fPabo',
  root_height: 578320,
  tip: '3NKtKuzrhiUG3Hp6KWhDAs1aAQd381Z7yqithKcsmYQdPx8BsT56',
  tip_height: 578610,
  blocks: 290,
  ledgers: 3,
  load_start: ns(t0),
  load_ms: 6_100.5,
  replay_start: ns(t0 + 6_500),
  replay_end: ns(t0 + 11_900),
};

test('a committed restore reads as one, with how long it took', () => {
  const r = restoreSummary(restored);
  assert.deepEqual(r, {
    tipHeight: 578610,
    rootHeight: 578320,
    blocks: 290,
    done: true,
    failed: false,
    failure: null,
    totalMs: 11_900,
    endedAt: t0 + 11_900,
  });
  assert.equal(
    restoreHeadline(r),
    'Restored from saved state at #578610 in 12 s',
  );
  assert.equal(restoreLedgersNote(r), 'restored at #578610 in 12 s');
});

test('while the replay runs, it says so', () => {
  const r = restoreSummary({ ...restored, replay_end: null });
  assert.equal(r.done, false);
  assert.equal(r.totalMs, null);
  assert.equal(
    restoreHeadline(r),
    'Restoring from saved state: replaying 290 blocks to #578610',
  );
  assert.equal(restoreLedgersNote(r), 'loaded from saved state');
});

test('a replay that failed never reads as replaying', () => {
  // The node's record once the replay failed at 02:00:09.000: no commit, and
  // why (SyncRestoreFailed).
  const r = restoreSummary({
    ...restored,
    replay_end: null,
    failed: {
      time: ns(t0 + 9_000),
      reason: 'block 3NKtKuzr... at 578400: staged ledger hash mismatch',
    },
  });
  assert.equal(r.failed, true);
  assert.equal(r.done, false);
  assert.equal(
    r.failure,
    'block 3NKtKuzr... at 578400: staged ledger hash mismatch',
  );
  const headline = restoreHeadline(r);
  assert.equal(
    headline,
    "Saved state couldn't be replayed; syncing from the network",
  );
  assert.doesNotMatch(headline, /replaying/i);
  assert.equal(restoreLedgersNote(r), 'saved state not used');
});

test('a record from before the failure field reads as before', () => {
  const r = restoreSummary({ ...restored, failed: undefined });
  assert.equal(r.failed, false);
  assert.equal(r.done, true);
});

test('a bootstrap, or an older node, has no restore', () => {
  assert.equal(restoreSummary(undefined), null);
  assert.equal(restoreSummary(null), null);
});

test('durations read as people write them', () => {
  assert.equal(restoreDuration(940), '0.9 s');
  assert.equal(restoreDuration(9_940), '9.9 s');
  assert.equal(restoreDuration(11_900), '12 s');
  assert.equal(restoreDuration(65_000), '1 min 5 s');
  assert.equal(restoreDuration(120_000), '2 min');
});
