/**
 * The payments this tab sent, and the look that follows them. Every change
 * is by transaction hash, against the list as it is when the change
 * arrives: a payment accepted while a look is out is never lost when the
 * look comes back. Looks don't overlap, so an older answer can't overwrite
 * a newer one.
 */

import type { Account, InclusionLook } from "./node";
import { nextStage, type Sent } from "./payment";

export class SentStore {
  private list: Sent[] = [];
  private listeners = new Set<() => void>();

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = () => this.list;

  private set(list: Sent[]) {
    this.list = list;
    for (const listener of this.listeners) listener();
  }

  /** A new payment, first; one with the same hash is replaced. */
  add(sent: Sent) {
    this.set([sent, ...this.list.filter(s => s.hash !== sent.hash)]);
  }

  /** Changes the payment with this hash as it is now, if it is still
   * here. */
  update(hash: string, change: (current: Sent) => Partial<Sent>) {
    const i = this.list.findIndex(s => s.hash === hash);
    if (i < 0) return;
    const next = [...this.list];
    next[i] = { ...next[i], ...change(next[i]) };
    this.set(next);
  }
}

export const sentStore = new SentStore();

export const isOpen = (s: Sent) =>
  s.stage === "pending" || s.stage === "included";

/** What a look asks the node. */
export interface TrackNode {
  account(publicKey: string): Promise<Account | null>;
  inclusion(hash: string, depth: number): Promise<InclusionLook | null>;
  inPool(hash: string): Promise<boolean>;
}

/** Blocks a look searches for a sent payment. */
export const SEARCH_DEPTH = 40;

/** One look at every open payment. A lookup that fails is unknown, and
 * changes nothing it would have decided. */
export async function trackOnce(
  node: TrackNode,
  store: SentStore,
  depth = SEARCH_DEPTH,
): Promise<void> {
  const open = store.getSnapshot().filter(isOpen);
  const nonces = new Map<string, Promise<number | null | undefined>>();
  const nonceOf = (from: string) => {
    let n = nonces.get(from);
    if (!n) {
      n = node.account(from).then(
        a => (a ? a.nonce : null),
        () => undefined,
      );
      nonces.set(from, n);
    }
    return n;
  };
  await Promise.all(
    open.map(async s => {
      const [inclusion, inPool, accountNonce] = await Promise.all([
        node.inclusion(s.hash, depth).catch(() => undefined),
        node.inPool(s.hash).catch(() => undefined),
        nonceOf(s.from),
      ]);
      store.update(s.hash, current =>
        nextStage(current, { inclusion, inPool, accountNonce }),
      );
    }),
  );
}

/** Looks every `everyMs` while there is an open payment, one look at a
 * time: a tick while a look is out is skipped. */
export class Tracker {
  private busy = false;

  constructor(
    private readonly node: TrackNode,
    private readonly store: SentStore,
  ) {}

  /** One look, unless one is out. Resolves when it is done. */
  async tick(): Promise<boolean> {
    if (this.busy) return false;
    this.busy = true;
    try {
      await trackOnce(this.node, this.store);
      return true;
    } finally {
      this.busy = false;
    }
  }
}
