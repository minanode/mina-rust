import { expect, test } from "vitest";

import { parseMemoryStats } from "./memory";

test("the heap counters, as memory_stats() returns them", () => {
  expect(
    parseMemoryStats({
      live_bytes: 512_000_000,
      peak_live_bytes: 900_000_000,
      wasm_bytes: 1_200_000_000,
    }),
  ).toEqual({
    liveBytes: 512_000_000,
    peakLiveBytes: 900_000_000,
    wasmBytes: 1_200_000_000,
  });
  expect(parseMemoryStats(undefined)).toBeNull();
  expect(parseMemoryStats({ wasm_bytes: 1 })).toBeNull();
});
