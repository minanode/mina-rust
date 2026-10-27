import { expect, test } from "vitest";

import {
  formatAge,
  formatDuration,
  formatHeight,
  formatMina,
  formatOffset,
  minaToNanomina,
  shortHash,
  utcDateTime,
} from "./format";

test("MINA amounts are exact", () => {
  expect(formatMina("21103243")).toBe("0.021103243");
  expect(formatMina("10000000")).toBe("0.01");
  expect(formatMina("11100000000")).toBe("11.1");
  expect(formatMina(0)).toBe("0");
  expect(formatMina("1000000000000000")).toBe("1,000,000");
  expect(formatMina(10_000_000n)).toBe("0.01");
  expect(formatMina(null)).toBeNull();
  expect(formatMina("-1")).toBeNull();
  expect(formatMina(1.5)).toBeNull();
});

test("MINA decimals to nanomina", () => {
  expect(minaToNanomina("0.01")).toBe(10_000_000n);
  expect(minaToNanomina("1")).toBe(1_000_000_000n);
  expect(minaToNanomina("0.000000001")).toBe(1n);
  expect(minaToNanomina("0.0000000001")).toBeNull();
  expect(minaToNanomina("1.")).toBeNull();
  expect(minaToNanomina("")).toBeNull();
});

test("ages, durations, heights and hashes", () => {
  expect(formatAge(45_000)).toBe("45 s");
  expect(formatAge(390_000)).toBe("6 min 30 s");
  expect(formatAge(7_500_000)).toBe("2 h 5 min");
  expect(formatAge(273_600_000)).toBe("3 d 4 h");
  expect(formatAge(-5)).toBe("0 s");
  expect(formatAge(null)).toBe("–");
  expect(formatDuration(940)).toBe("0.9 s");
  expect(formatDuration(11_900)).toBe("12 s");
  expect(formatDuration(65_000)).toBe("1 min 5 s");
  expect(formatOffset(3_900)).toBe("+3.9 s");
  expect(formatOffset(-5_750)).toBe("−5.8 s");
  expect(formatOffset(-2_650_240)).toBe("44 min 10 s before");
  expect(formatOffset(120_000)).toBe("2 min after");
  expect(formatOffset(null)).toBe("–");
  expect(formatHeight(578883)).toBe("#578883");
  expect(formatHeight(null)).toBe("–");
  expect(
    shortHash("B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t"),
  ).toBe("B62qp3…MNXb2t");
  expect(shortHash("short")).toBe("short");
  expect(utcDateTime(Date.UTC(2026, 9, 23, 16, 9))).toBe(
    "2026-10-23 16:09 UTC",
  );
});
