import { describe, expect, test } from "vitest";

import recorded from "./fixtures/producer-attempts-devnet-3610.json";
import { parseProducerStats, upcomingWonSlots, type Attempt } from "./producer";
import {
  attemptReport,
  attemptReports,
  binomialRange,
  epochExpectations,
  productionSummary,
  reportsStake,
  slotDurationMs,
  winProbability,
} from "./production";

// Recorded from a devnet producer (port 3610), the slots ahead cut to 24:
// three canonical blocks, three that lost the tie-break, one orphaned, and
// the next won slot scheduled. Its uptime then was long enough to include
// them all.
const stats = parseProducerStats(recorded)!;
const UPTIME = 7 * 3600;

function bySlot(slot: number) {
  return attemptReports(stats, UPTIME).find(
    r => r.attempt.wonSlot.globalSlot === slot,
  )!;
}

test("devnet slots are 90 s, as the node's slot times show", () => {
  expect(slotDurationMs(stats)).toBe(90_000);
});

test("each recorded attempt has its outcome in words", () => {
  const outcomes = attemptReports(stats, UPTIME).map(r => [
    r.attempt.wonSlot.globalSlot,
    r.outcome,
  ]);
  expect(outcomes).toEqual([
    [62780, "upcoming"],
    [62777, "orphaned"],
    [62773, "lostTie"],
    [62767, "lostTie"],
    [62764, "canonical"],
    [62750, "canonical"],
    [62736, "canonical"],
    [62720, "lostTie"],
  ]);
  // Confirmed from 15 blocks on top, as included SNARK work is.
  expect(bySlot(62736).label).toBe("Confirmed");
  expect(bySlot(62736).reason).toBe(
    "In the best chain with 19 blocks on top when last seen.",
  );
  expect(bySlot(62750).label).toBe("Pending");
  expect(bySlot(62750).reason).toBe(
    "In the best chain with 12 blocks on top when last seen; confirmed at 15.",
  );
  expect(bySlot(62773).label).toBe("Lost the tie");
  expect(bySlot(62773).reason).toMatch(/same key, the larger hash wins/);
  expect(bySlot(62777).reason).toMatch(/#579109/);
});

test("a canonical block's stages, against its slot", () => {
  const r = bySlot(62736);
  expect(r.stages.map(s => s.state)).toEqual(Array(7).fill("done"));
  const offset = Object.fromEntries(
    r.stages.map(s => [s.key, Math.round((s.offsetMs ?? NaN) / 100) / 10]),
  );
  // Built and proving from 5.75 s before the slot; out 3.9 s into it.
  expect(offset).toMatchObject({
    built: -5.7,
    proofStart: -5.7,
    proofEnd: 2.8,
    injected: 3.1,
    applied: 3.8,
    broadcast: 3.9,
  });
  expect(r.proofMs).toBeCloseTo(8592, -1);
  expect(r.broadcastOffsetMs).toBeCloseTo(3900, -2);
  expect(r.sent).toBe(true);
});

test("a lost tie stops at the proof and is never sent", () => {
  const r = bySlot(62773);
  expect(r.stages.map(s => s.state)).toEqual([
    "done",
    "done",
    "done",
    "stopped",
    "skipped",
    "skipped",
    "skipped",
  ]);
  expect(r.sent).toBe(false);
  expect(r.proofMs).toBeNull();
  expect(r.discardedOffsetMs).toBeCloseTo(1500, -2);
});

test("the scheduled slot is upcoming, its stages to come", () => {
  const r = bySlot(62780);
  expect(r.label).toBe("Scheduled");
  expect(r.stages.map(s => s.state)).toEqual([
    "done",
    "current",
    "pending",
    "pending",
    "pending",
    "pending",
    "pending",
  ]);
});

test("the summary counts each outcome once", () => {
  const reports = attemptReports(stats, UPTIME);
  const s = productionSummary(reports, upcomingWonSlots(stats));
  expect(s).toMatchObject({
    won: 7,
    // The scheduled slot and the 24 listed ahead.
    ahead: 25,
    produced: 4,
    canonical: 3,
    orphaned: 1,
    lostTie: 3,
    missed: 0,
    producing: 0,
    coinbase: 3 * 360_000_000_000,
  });
  // Four proofs: 8.6, 9.0, 9.1 and 9.2 s.
  expect(s.proof?.count).toBe(4);
  expect(s.proof?.median).toBeCloseTo(9050, -2);
  expect(s.proof?.worst).toBeCloseTo(9240, -2);
  // Out 3.4 to 4.2 s into the slot.
  expect(s.broadcast?.median).toBeCloseTo(3670, -2);
  expect(s.broadcast?.worst).toBeCloseTo(4180, -2);
  expect(s.sinceMs).toBe(stats.attempts.at(-1)!.wonSlot.slotTimeMs);
});

describe("attempts from before the node's latest start", () => {
  const slotStart = stats.attempts.find(a => a.wonSlot.globalSlot === 62780)!;
  const later = (slotStart.wonSlot.slotTimeMs ?? 0) + 10 * 60_000;
  const ctx = { nowMs: later, startedMs: later - 60_000 };

  test("a scheduled slot that passed while the node was down is missed", () => {
    const r = attemptReport(slotStart, ctx);
    expect(r).toMatchObject({ outcome: "missed", interrupted: true });
    expect(r.reason).toMatch(/restarted before the slot came/);
  });

  test("a half-made block is missed, interrupted", () => {
    const half: Attempt = { ...slotStart, status: "ProofCreatePending" };
    expect(attemptReport(half, ctx)).toMatchObject({
      outcome: "missed",
      interrupted: true,
    });
    // In this run it is still being made.
    expect(attemptReport(half, { ...ctx, startedMs: null }).outcome).toBe(
      "producing",
    );
  });

  test("a sent block whose fate the node didn't see says so", () => {
    const sent = stats.attempts.find(a => a.status === "Canonical")!;
    const r = attemptReport(
      { ...sent, status: "Committed", confirmations: null },
      { nowMs: later, startedMs: later - 60_000 },
    );
    expect(r).toMatchObject({ outcome: "broadcast", sent: true });
    expect(r.reason).toMatch(/restarted before it saw/);
  });
});

describe("a restart just before a won slot", () => {
  // Restarted 60 s ago: slot 62780's attempt, scheduled 266 s before the
  // slot, is the last run's; the slot starts in 77 s.
  const UP = 60;
  const startedMs = stats.currentTimeMs! - UP * 1000;

  test("the last run's scheduled attempt is shown as such, not as active", () => {
    const r = bySlotAt(62780, stats);
    expect(r).toMatchObject({
      outcome: "previousRun",
      label: "From before the restart",
      previousRun: true,
      ended: false,
    });
    expect(r.reason).toMatch(/hasn't found the slot/);
    // Not the next won slot: this run hasn't scheduled it.
    expect(upcomingWonSlots(stats, startedMs)[0]).toMatchObject({
      globalSlot: stats.futureWonSlots[0].globalSlot,
      scheduled: false,
    });
    expect(stats.futureWonSlots[0].globalSlot).toBe(62793);
    const s = productionSummary(attemptReports(stats, UP), []);
    expect(s.won).toBe(7);
  });

  test("once this run finds the slot, it says it will schedule it", () => {
    const found = { ...stats, futureWonSlots: [scheduled.wonSlot] };
    expect(bySlotAt(62780, found).reason).toMatch(/found the slot too/);
  });

  test("once this run schedules the slot again, its attempt replaces the last run's", () => {
    const again = {
      ...scheduled,
      times: { ...scheduled.times, scheduled: stats.currentTimeMs! - 10_000 },
    };
    const rescheduled = { ...stats, attempts: [again, ...stats.attempts] };
    const reports = attemptReports(rescheduled, UP).filter(
      r => r.attempt.wonSlot.globalSlot === 62780,
    );
    expect(reports).toHaveLength(1);
    expect(reports[0]).toMatchObject({
      outcome: "upcoming",
      previousRun: false,
    });
    expect(upcomingWonSlots(rescheduled, startedMs)[0]).toMatchObject({
      globalSlot: 62780,
      scheduled: true,
    });
  });

  test("a block the restart stopped, for a slot not over, is from before it too", () => {
    const proving = { ...scheduled, status: "ProofCreatePending" };
    const r = attemptReport(proving, {
      nowMs: stats.currentTimeMs!,
      startedMs,
    });
    expect(r).toMatchObject({ outcome: "previousRun", interrupted: false });
  });

  const scheduled = stats.attempts.find(a => a.status === "Scheduled")!;
  function bySlotAt(slot: number, s: typeof stats) {
    return attemptReports(s, UP).find(
      r => r.attempt.wonSlot.globalSlot === slot,
    )!;
  }
});

test("an old build reports no stake: no expected wins", () => {
  expect(reportsStake(stats.vrfEpochs)).toBe(false);
  expect(epochExpectations(stats.vrfEpochs)).toEqual([]);
  expect(stats.vrfEpochs.map(e => [e.epoch, e.evaluatedSlots])).toEqual([
    [8, 1569],
    [9, 7140],
  ]);
});

test("expected wins from the stake, against those found", () => {
  const withStake = parseProducerStats({
    ...recorded,
    vrf_stats: {
      "9": {
        total_slots: 7140,
        evaluated_slots: 7140,
        won_slots: 449,
        // 8.2% of the stake.
        delegated_stake: 82_000_000_000_000_000,
        total_currency: 1_000_000_000_000_000_000,
        delegators: 3,
      },
    },
  })!;
  expect(reportsStake(withStake.vrfEpochs)).toBe(true);
  const [e] = epochExpectations(withStake.vrfEpochs);
  expect(e.share).toBeCloseTo(0.082, 6);
  expect(e.perSlot).toBeCloseTo(1 - Math.pow(0.25, 0.082), 9);
  expect(e.expected).toBeCloseTo(7140 * e.perSlot, 6);
  expect(e.expected).toBeCloseTo(767.2, 1);
  // The binomial's central 95%: about two standard deviations (26) either
  // side with this many wins.
  expect(e.low).toBeGreaterThan(710);
  expect(e.low).toBeLessThan(720);
  expect(e.high).toBeGreaterThan(815);
  expect(e.high).toBeLessThan(825);
  expect(e.found).toBe(449);
});

test("the chance of a win per slot", () => {
  expect(winProbability(1, 1)).toBeCloseTo(0.75, 12);
  expect(winProbability(0, 1)).toBe(0);
  expect(winProbability(1, 0)).toBe(0);
  expect(winProbability(1, 100)).toBeCloseTo(0.013767, 6);
});

/** P(X = k) for X ~ Binomial(n, p), directly: for small n only. */
function pmf(n: number, p: number, k: number): number {
  let c = 1;
  for (let i = 0; i < k; i++) c = (c * (n - i)) / (i + 1);
  return c * Math.pow(p, k) * Math.pow(1 - p, n - k);
}

test("the 95% range is exact and tight, for few slots and few wins", () => {
  // 20 slots at 5%: one win expected; 0 to 3 holds 98%, and 0 to 2 only
  // 92%, so the range is 0 to 3.
  expect(binomialRange(20, 0.05)).toEqual([0, 3]);
  for (const [n, p] of [
    [20, 0.05],
    [10, 0.5],
    [50, 0.013767],
    [100, 0.2],
    [300, 0.01],
  ]) {
    const [low, high] = binomialRange(n, p);
    let below = 0;
    for (let k = 0; k < low; k++) below += pmf(n, p, k);
    let above = 0;
    for (let k = high + 1; k <= n; k++) above += pmf(n, p, k);
    expect(below).toBeLessThanOrEqual(0.025 + 1e-12);
    expect(above).toBeLessThanOrEqual(0.025 + 1e-12);
    // Tight: one step in on either side leaves more than 2.5% out.
    if (low > 0) expect(below + pmf(n, p, low)).toBeGreaterThan(0.025);
    if (high < n) expect(above + pmf(n, p, high)).toBeGreaterThan(0.025);
  }
  expect(binomialRange(0, 0.1)).toEqual([0, 0]);
  expect(binomialRange(100, 0)).toEqual([0, 0]);
});
