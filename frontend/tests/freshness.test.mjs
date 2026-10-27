import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  formatAge,
  formatHeight,
  freshness,
  nanosToMs,
  peerNote,
  shownStatus,
  statusChip,
  STATUS_STALE_MS,
  TIP_QUIET_MS,
  utcTime,
} from '../src/app/shared/helpers/freshness.helper.ts';

// R26c: three clocks, three truthful states. The times are R26's observation
// of the native header: block 564682 made at 07:13:30, its sync event at
// 07:19:26, the page at 07:20:29 UTC.
const at = hms => Date.UTC(2026, 9, 21, ...hms.split(':').map(Number));
const now = at('07:20:29');
const synced = {
  status: 'Synced',
  blockHeight: 564682,
  bestTipTimestamp: at('07:13:30'),
  syncTime: at('07:19:26'),
  sampledAt: at('07:20:26'),
  maxPeerHeight: 564682,
};

test('a fixed older block shows its own age, not the sync event', () => {
  const f = freshness(synced, now);
  assert.equal(formatAge(f.tipAgeMs), '6 min 59 s');
  assert.equal(formatAge(f.syncEventAgeMs), '1 min 3 s');
  assert.equal(formatAge(f.sampleAgeMs), '3 s');
  assert.equal(f.sampleStale, false);
  assert.equal(f.tipQuiet, false);
  assert.equal(f.behindPeers, 0);
  assert.deepEqual(statusChip(synced, now), {
    label: 'Synced',
    height: '#564682',
    detail: 'block 6 min 59 s old',
    tone: 'ok',
  });
});

test('a new sync event does not make an old block look fresh', () => {
  const f = freshness({ ...synced, syncTime: now }, now);
  assert.equal(f.syncEventAgeMs, 0);
  assert.equal(formatAge(f.tipAgeMs), '6 min 59 s');
});

test('a stale API sample is visibly stale, not a fresh green chip', () => {
  const later = synced.sampledAt + STATUS_STALE_MS + 15_000;
  const chip = statusChip(synced, later);
  assert.equal(chip.tone, 'warn');
  assert.equal(chip.detail, 'no answer for 45 s');
  assert.equal(freshness(synced, later).sampleStale, true);
  const offline = statusChip({ ...synced, status: 'Offline' }, later);
  assert.equal(offline.label, 'Offline');
  assert.equal(offline.detail, 'last answer 45 s ago');
  assert.equal(offline.tone, 'warn');
});

test('a long quiet chain is flagged as quiet, not as a fault', () => {
  const quiet = synced.bestTipTimestamp + TIP_QUIET_MS + 60_000;
  const chip = statusChip({ ...synced, sampledAt: quiet }, quiet);
  assert.equal(chip.tone, 'aware');
  assert.equal(chip.label, 'Synced');
});

test('bootstrap never says Synced or shows the genesis block as the tip', () => {
  const chip = statusChip(
    {
      status: 'Bootstrap',
      blockHeight: 548147,
      bestTipTimestamp: at('07:00:00'),
      targetHeight: 564686,
      sampledAt: now,
    },
    now,
  );
  assert.deepEqual(chip, {
    label: 'Bootstrapping',
    height: 'to #564686',
    detail: 'not synced yet',
    tone: 'aware',
  });
  assert.equal(statusChip({ status: 'Pending' }, now).label, 'Starting');
  assert.equal(
    statusChip({ status: 'Catchup', targetHeight: 7, sampledAt: now }, now)
      .label,
    'Catching up',
  );
});

test('Synced on the genesis block at start is not shown as synced', () => {
  // R26 builds run: the node reported Synced at #548147, Mesa's genesis,
  // for about 10 s before a peer's tip arrived.
  const genesis = {
    status: 'Synced',
    blockHeight: 548147,
    bestTipTimestamp: now - 3 * 86_400_000,
    sampledAt: now,
  };
  const f = freshness(genesis, now);
  assert.equal(f.synced, false);
  assert.equal(f.tipQuiet, false);
  assert.equal(shownStatus(genesis, now), 'Not synced');
  assert.deepEqual(statusChip(genesis, now), {
    label: 'Not synced',
    height: '#548147',
    detail: 'block 3 d old',
    tone: 'aware',
  });
  // An older node without block timestamps: peers far ahead tell.
  const behind = {
    status: 'Synced',
    blockHeight: 548147,
    maxPeerHeight: 564712,
    sampledAt: now,
  };
  assert.equal(statusChip(behind, now).detail, 'peers at #564712');
  assert.equal(shownStatus(behind, now), 'Not synced');
  // One or two blocks behind a peer is ordinary propagation.
  assert.equal(
    freshness({ ...synced, maxPeerHeight: 564684 }, now).synced,
    true,
  );
  assert.equal(shownStatus(synced, now), 'Synced');
  assert.equal(shownStatus({ status: 'Bootstrap' }, now), 'Bootstrapping');
});

test('Synced with no block time and no peer tip is not shown as synced', () => {
  // Review of R26c: an m6 node reports no block timestamp, so on genesis,
  // before any peer connects (as after a watchdog restart), nothing tells.
  const bare = { status: 'Synced', blockHeight: 548147, sampledAt: now };
  assert.equal(freshness(bare, now).synced, false);
  assert.equal(shownStatus(bare, now), 'Not synced');
  assert.deepEqual(statusChip(bare, now), {
    label: 'Not synced',
    height: '#548147',
    detail: 'no block time or peer tip to check',
    tone: 'aware',
  });
  // Either clue is enough: a peer level with us, or a recent block time.
  assert.equal(
    freshness({ ...bare, blockHeight: 564682, maxPeerHeight: 564682 }, now)
      .synced,
    true,
  );
  assert.equal(
    freshness({ ...bare, bestTipTimestamp: at('07:13:30') }, now).synced,
    true,
  );
});

test('units and edge cases', () => {
  assert.equal(nanosToMs(1_761_030_810_000_000_000), 1_761_030_810_000);
  assert.equal(nanosToMs(null), null);
  assert.equal(nanosToMs(0), null);
  assert.equal(formatAge(null), '–');
  assert.equal(formatAge(59_999), '59 s');
  assert.equal(formatAge(3_600_000), '1 h');
  assert.equal(formatAge(90_000_000), '1 d 1 h');
  assert.equal(utcTime(at('07:13:30')), '07:13:30 UTC');
  // A peer clock ahead of ours is an age of zero, not negative.
  assert.equal(
    freshness({ ...synced, bestTipTimestamp: now + 5000 }, now).tipAgeMs,
    0,
  );
  // Peers ahead, and no peer to compare with.
  assert.equal(
    freshness({ ...synced, maxPeerHeight: 564684 }, now).behindPeers,
    2,
  );
  assert.equal(
    freshness({ ...synced, maxPeerHeight: null }, now).behindPeers,
    null,
  );
});

test('a peer known only by genesis is below us, not level', () => {
  // R26 builds run 3: relay2 reconnected and was known by #548147 again.
  const f = freshness({ ...synced, maxPeerHeight: 548147 }, now);
  assert.equal(f.peerDelta, -16535);
  assert.equal(f.synced, true);
  assert.equal(peerNote(f), '16535 blocks below ours: peer tip not known yet');
  assert.equal(peerNote(freshness(synced, now)), 'level with our best tip');
  assert.equal(
    peerNote(freshness({ ...synced, maxPeerHeight: 564683 }, now)),
    '1 block above ours',
  );
  assert.equal(
    peerNote(freshness({ ...synced, maxPeerHeight: null }, now)),
    'no peer tip to compare',
  );
});

test('one block-height format on the dashboard (F-dash2)', () => {
  // The design critique found #578486, 578486 and #578,486 on one page.
  // #578,486 runs past the sync card's edge on a 390 px phone.
  assert.equal(formatHeight(578486), '#578486');
  assert.equal(formatHeight(1), '#1');
  assert.equal(formatHeight(1234567), '#1234567');
  for (const unknown of [null, undefined, 0, NaN]) {
    assert.equal(formatHeight(unknown), '–');
  }
});

test('before a peer sends a best tip, the node says so (F-dash3)', () => {
  // After a restart, from the genesis block or from saved state, the node
  // reports Connecting (no peer) or Listening (no best tip yet), not Synced.
  const connecting = { ...synced, status: 'Connecting', maxPeerHeight: null };
  assert.deepEqual(statusChip(connecting, now), {
    label: 'Connecting',
    height: '',
    detail: 'no peer yet',
    tone: 'aware',
  });
  assert.equal(shownStatus(connecting, now), 'Connecting');
  const listening = { ...synced, status: 'Listening' };
  assert.deepEqual(statusChip(listening, now), {
    label: 'Waiting for peers',
    height: '',
    detail: 'no best tip from peers yet',
    tone: 'aware',
  });
  assert.equal(shownStatus(listening, now), 'Waiting for peers');
  assert.equal(freshness(listening, now).synced, false);
  const stale = statusChip(
    { ...listening, sampledAt: now - STATUS_STALE_MS - 15_000 },
    now,
  );
  assert.equal(stale.tone, 'warn');
  assert.equal(stale.detail, 'no answer for 45 s');
});
