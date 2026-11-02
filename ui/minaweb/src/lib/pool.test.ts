import { expect, test } from "vitest";

import type { PendingTransaction } from "@minanode/node-client";

import { poolStatuses, queuedSenders } from "./pool";

const tx = (
  sender: string | null,
  nonce: number | null,
): PendingTransaction => ({
  hash: `${sender}-${nonce}`,
  kind: "Payment",
  sender,
  receiver: null,
  amount: 1n,
  fee: 1n,
  nonce,
  memo: null,
});

test("a sender's lowest nonce goes next; the rest wait behind it", () => {
  const list = [tx("a", 5), tx("b", 1), tx("a", 4), tx("a", 6), tx(null, 2)];
  expect(poolStatuses(list)).toEqual([
    { kind: "queued", after: 4 },
    { kind: "next" },
    { kind: "next" },
    { kind: "queued", after: 5 },
    { kind: "unknown" },
  ]);
  expect(queuedSenders(list)).toBe(1);
});
