import { expect, test } from "vitest";

import { frontierStoreChoice, ledgerDetail } from "./boot";

test("the ledger step names the ledger it fetches", () => {
  expect(
    ledgerDetail({
      stakingEpoch: { done: 240_000, total: 240_000 },
      nextEpoch: { done: 1_000, total: 240_000 },
      root: null,
      rootStaged: null,
    }),
  ).toBe("Next epoch ledger: 1,000 of about 240,000 accounts.");
  expect(
    ledgerDetail({
      stakingEpoch: { done: 2, total: 2 },
      nextEpoch: { done: 2, total: 2 },
      root: { done: 2, total: 2 },
      rootStaged: { done: 1, total: 4 },
    }),
  ).toBe("Root staged ledger: 1 of 4 parts.");
  expect(
    ledgerDetail({
      stakingEpoch: null,
      nextEpoch: null,
      root: null,
      rootStaged: null,
    }),
  ).toBe("Asking peers for the ledgers.");
});

test("a stored choice overrides the build's frontier store", () => {
  expect(frontierStoreChoice(true, null)).toBe(true);
  expect(frontierStoreChoice(true, "off")).toBe(false);
  expect(frontierStoreChoice(false, "on")).toBe(true);
  expect(frontierStoreChoice(false, "maybe")).toBe(false);
});
