import { expect, test } from "vitest";

import recorded from "./fixtures/producer-devnet-3610.json";
import { parseProducerStats, upcomingWonSlots } from "./producer";

test("the scheduled won slot comes first among the slots ahead", () => {
  // Recorded from a devnet producer: slot 62508 is Scheduled (so it left
  // future_won_slots), 62514 is the first of future_won_slots.
  const stats = parseProducerStats(recorded)!;
  expect(stats.futureWonSlots[0].globalSlot).toBe(62514);
  const scheduled = stats.attempts.find(a => a.status === "Scheduled")!;
  expect(scheduled.wonSlot.globalSlot).toBe(62508);
  const ahead = upcomingWonSlots(stats);
  expect(ahead[0]).toMatchObject({ globalSlot: 62508, scheduled: true });
  expect(ahead[1]).toMatchObject({ globalSlot: 62514, scheduled: false });
  expect(ahead).toHaveLength(stats.futureWonSlots.length + 1);
  const times = ahead.map(w => w.slotTimeMs ?? 0);
  expect(times).toEqual([...times].sort((a, b) => a - b));
});

test("a slot both scheduled and listed ahead appears once", () => {
  const stats = parseProducerStats(recorded)!;
  const twice = {
    ...stats,
    futureWonSlots: [
      stats.attempts.find(a => a.status === "Scheduled")!.wonSlot,
      ...stats.futureWonSlots,
    ],
  };
  expect(upcomingWonSlots(twice)).toHaveLength(stats.futureWonSlots.length + 1);
});
