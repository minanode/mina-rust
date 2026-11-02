import { expect, test } from "vitest";

import { lowMemoryWarning } from "./resources";

test("a device that reports little memory gets a warning, not a refusal", () => {
  expect(lowMemoryWarning("producer", null)).toBeNull();
  expect(lowMemoryWarning("producer", 8)).toBeNull();
  expect(lowMemoryWarning("worker", 4)).toMatch(/You can still start it/);
  expect(lowMemoryWarning("producer", 2)).toMatch(/2 GB/);
});
