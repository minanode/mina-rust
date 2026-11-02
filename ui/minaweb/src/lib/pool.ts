/** The node's view of each pending transaction. The pool holds only
 * commands it has checked (signature, fee, nonce against the ledger). A
 * block takes a sender's commands in nonce order, so only the lowest nonce
 * of each sender can go into the next block; the rest wait behind it. */

import type { PendingTransaction } from "@minanode/node-client";

export type PoolStatus =
  | { kind: "next" }
  | { kind: "queued"; after: number }
  | { kind: "unknown" };

/** By transaction: keyed by the list's index. */
export function poolStatuses(list: PendingTransaction[]): PoolStatus[] {
  const lowest = new Map<string, number>();
  for (const t of list) {
    if (t.sender === null || t.nonce === null) continue;
    const seen = lowest.get(t.sender);
    if (seen === undefined || t.nonce < seen) lowest.set(t.sender, t.nonce);
  }
  return list.map(t => {
    if (t.sender === null || t.nonce === null) return { kind: "unknown" };
    const first = lowest.get(t.sender)!;
    return t.nonce === first
      ? { kind: "next" }
      : { kind: "queued", after: t.nonce - 1 };
  });
}

/** Senders with more than one pending command. */
export function queuedSenders(list: PendingTransaction[]): number {
  const counts = new Map<string, number>();
  for (const t of list) {
    if (t.sender) counts.set(t.sender, (counts.get(t.sender) ?? 0) + 1);
  }
  return [...counts.values()].filter(n => n > 1).length;
}
