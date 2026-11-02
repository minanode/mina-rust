import { describe, expect, test } from "vitest";

import { DAY_MS, days, epochClock, stakeVerdict } from "./epoch";
import type { Stake } from "./node";

// Devnet's constants since Mesa: 7,140 slots of 90 s, as the node reports.
const consensus = { slotsPerEpoch: 7140, slotMs: 90_000, k: 290 };
const EPOCH_MS = 7140 * 90_000;

describe("when delegated stake counts", () => {
  test("earliest is the start of the epoch after next", () => {
    const now = 1_800_000_000_000;
    // Slot 1,000 of epoch 20 began now.
    const clock = epochClock(consensus, {
      epoch: 20,
      globalSlot: 20 * 7140 + 1000,
      slot: 1000,
      slotStartMs: now,
    });
    expect(clock.epochMs).toBe(EPOCH_MS);
    expect(clock.epochEndMs).toBe(now + 6140 * 90_000);
    expect(clock.delegationEpoch).toBe(22);
    expect(clock.delegationCountsFromMs).toBe(now + 6140 * 90_000 + EPOCH_MS);
    // An epoch is 7.4 days; this delegation counts in 13.8, shown as 14.
    expect(days(EPOCH_MS)).toBe("7.4 days");
    expect(days(clock.delegationCountsFromMs - now)).toBe("14 days");
  });

  test("days, to one decimal below ten", () => {
    expect(days(DAY_MS)).toBe("1.0 day");
    expect(days(12.4 * DAY_MS)).toBe("12 days");
    expect(days(-5)).toBe("0.0 days");
  });
});

const ledger = (stake: bigint | null) => ({
  hash: "jx",
  delegatedStake: stake,
  delegators: stake === null ? null : 1,
  totalCurrency: 1_000_000_000_000_000n,
});
const stake = (now: bigint | null, next: bigint | null): Stake => ({
  epoch: 20,
  bestTipHeight: 1,
  staking: ledger(now),
  next: ledger(next),
});
const clock = epochClock(consensus, {
  epoch: 20,
  globalSlot: 0,
  slot: 0,
  slotStartMs: 0,
});

describe("what the stake means", () => {
  test("no stake: it will not produce", () => {
    expect(stakeVerdict(stake(0n, 0n), clock, consensus)).toEqual({
      kind: "none",
    });
  });

  test("next epoch only: from its start", () => {
    expect(stakeVerdict(stake(0n, 5n), clock, consensus)).toEqual({
      kind: "next",
      fromMs: clock.epochEndMs,
      epoch: 21,
    });
  });

  test("stake now: the slots it can expect", () => {
    // 1% of the stake: 1 - 0.25^0.01 of 7,140 slots, about 98.
    const v = stakeVerdict(stake(10_000_000_000_000n, 0n), clock, consensus);
    expect(v.kind).toBe("now");
    if (v.kind === "now") expect(Math.round(v.expectedPerEpoch)).toBe(98);
  });

  test("no stake now and the next ledger unread: unknown, not none", () => {
    expect(stakeVerdict(stake(0n, null), clock, consensus).kind).toBe(
      "unknown",
    );
  });

  test("ledgers not read yet", () => {
    expect(stakeVerdict(stake(null, null), clock, consensus).kind).toBe(
      "unknown",
    );
    expect(stakeVerdict(null, clock, consensus).kind).toBe("unknown");
  });
});

test("the last slot and first slot straddle the earliest activation epoch", () => {
  const last = epochClock(consensus, {
    epoch: 20,
    globalSlot: 21 * 7140 - 1,
    slot: 7139,
    slotStartMs: 0,
  });
  const first = epochClock(consensus, {
    epoch: 21,
    globalSlot: 21 * 7140,
    slot: 0,
    slotStartMs: 90_000,
  });
  expect(last.delegationEpoch).toBe(22);
  expect(last.delegationCountsFromMs).toBe(90_000 + EPOCH_MS);
  expect(first.delegationEpoch).toBe(23);
  expect(first.delegationCountsFromMs).toBe(
    last.delegationCountsFromMs + EPOCH_MS,
  );
  expect(stakeVerdict(stake(5n, null), clock, consensus).kind).toBe("now");
});
