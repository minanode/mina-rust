/**
 * The payments this tab sent, and the look that follows them. Every change
 * is by transaction hash, against the list as it is when the change
 * arrives: a payment accepted while a look is out is never lost when the
 * look comes back. Looks don't overlap, so an older answer can't overwrite
 * a newer one.
 *
 * The list is kept in this tab's sessionStorage, so it outlasts the role
 * change's restart (a reload) and a reload of the visitor's own. It holds
 * the public fields the list shows and the tracking needs, never a key or
 * the memo: at most SENT_MAX payments, and none that is done and older
 * than SENT_KEEP_MS.
 */

import { num, obj, str } from "@minanode/node-client";

import { looksLikePublicKey } from "./keys";
import type { Account, Inclusion, InclusionLook } from "./node";
import { nextStage, type Sent, type SentStage } from "./payment";

/** sessionStorage: this tab's sent payments. */
export const SENT_KEY = "minaweb.sent";
/** The most payments the list keeps, newest first. */
export const SENT_MAX = 20;
/** A payment that is done (confirmed, failed, dropped or refused) leaves
 * the list this long after it was sent. */
export const SENT_KEEP_MS = 24 * 60 * 60_000;

const STAGES: readonly SentStage[] = [
  "pending",
  "included",
  "confirmed",
  "failed",
  "dropped",
  "rejected",
];
const HASH = /^[0-9A-Za-z]{1,64}$/;
const NANOMINA = /^\d{1,20}$/;
/** Longer than any detail this page writes. */
const DETAIL_MAX = 500;

/** Where a store keeps its list: null keeps it in memory only. */
export type SentStorage = () => Storage | null;

function tabStorage(): Storage | null {
  try {
    return sessionStorage;
  } catch {
    return null;
  }
}

/** The list without what is done and old, and at most SENT_MAX long. */
export function pruneSent(list: readonly Sent[], now: number): Sent[] {
  return list
    .filter(s => isOpen(s) || now - s.sentAt <= SENT_KEEP_MS)
    .slice(0, SENT_MAX);
}

/** The fields kept, by name: a field added to Sent stays out until it is
 * added here. */
function stored(s: Sent) {
  return {
    hash: s.hash,
    from: s.from,
    to: s.to,
    amount: s.amount.toString(),
    fee: s.fee.toString(),
    nonce: s.nonce,
    sentAt: s.sentAt,
    stage: s.stage,
    detail: s.detail,
    inclusion: s.inclusion && {
      height: s.inclusion.height,
      blockHash: s.inclusion.blockHash,
      depth: s.inclusion.depth,
      failure: s.inclusion.failure,
    },
  };
}

const publicKey = (value: unknown): string | null => {
  const v = str(value);
  return v !== null && v === v.trim() && looksLikePublicKey(v) ? v : null;
};

const count = (value: unknown): number | null => {
  const n = typeof value === "number" ? num(value) : null;
  return n !== null && Number.isSafeInteger(n) && n >= 0 ? n : null;
};

const nanomina = (value: unknown): bigint | null => {
  const v = str(value);
  return v !== null && NANOMINA.test(v) ? BigInt(v) : null;
};

function parseInclusion(raw: unknown): Inclusion | null | undefined {
  if (raw === null) return null;
  const i = obj(raw);
  const height = count(i?.height);
  const depth = count(i?.depth);
  const blockHash = str(i?.blockHash);
  const failure = i?.failure === null ? null : str(i?.failure);
  if (
    height === null ||
    depth === null ||
    blockHash === null ||
    !HASH.test(blockHash) ||
    failure === undefined ||
    (failure !== null && failure.length > DETAIL_MAX)
  ) {
    return undefined;
  }
  return { height, blockHash, depth, failure };
}

function parseOne(raw: unknown): Sent | null {
  const s = obj(raw);
  const hash = str(s?.hash);
  const from = publicKey(s?.from);
  const to = publicKey(s?.to);
  const amount = nanomina(s?.amount);
  const fee = nanomina(s?.fee);
  const nonce = count(s?.nonce);
  const sentAt = count(s?.sentAt);
  const stage = STAGES.find(x => x === s?.stage);
  const detail = s?.detail === null ? null : str(s?.detail);
  const inclusion = parseInclusion(s?.inclusion);
  if (
    hash === null ||
    !HASH.test(hash) ||
    from === null ||
    to === null ||
    amount === null ||
    fee === null ||
    nonce === null ||
    sentAt === null ||
    stage === undefined ||
    detail === undefined ||
    (detail !== null && detail.length > DETAIL_MAX) ||
    inclusion === undefined
  ) {
    return null;
  }
  return {
    hash,
    from,
    to,
    amount,
    fee,
    nonce,
    memo: "",
    sentAt,
    stage,
    detail,
    inclusion,
  };
}

/** A stored list, read without trusting it: what doesn't parse is left
 * out, and a hash is kept once. */
export function parseSent(raw: string | null): Sent[] {
  if (raw === null) return [];
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const list: Sent[] = [];
  for (const item of value.slice(0, SENT_MAX)) {
    const s = parseOne(item);
    if (!s || seen.has(s.hash)) continue;
    seen.add(s.hash);
    list.push(s);
  }
  return list;
}

export class SentStore {
  private list: Sent[];
  private listeners = new Set<() => void>();

  /** `storage` null: in memory only, as the tests use it. */
  constructor(
    private readonly storage: SentStorage | null = null,
    private readonly now: () => number = Date.now,
  ) {
    let raw: string | null = null;
    try {
      raw = storage?.()?.getItem(SENT_KEY) ?? null;
    } catch {
      // No storage in this tab: the list starts empty.
    }
    this.list = pruneSent(parseSent(raw), now());
    // Drops from storage what didn't parse or is done and old.
    if (raw !== null) this.save();
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = () => this.list;

  private set(list: Sent[]) {
    this.list = pruneSent(list, this.now());
    this.save();
    for (const listener of this.listeners) listener();
  }

  /** Writes the list, or removes it once empty. A tab whose storage is
   * off or full keeps the list in memory only. */
  private save() {
    try {
      const s = this.storage?.();
      if (!s) return;
      if (this.list.length === 0) s.removeItem(SENT_KEY);
      else s.setItem(SENT_KEY, JSON.stringify(this.list.map(stored)));
    } catch {
      // Kept in memory until the tab restarts.
    }
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

export const sentStore = new SentStore(tabStorage);

export function isOpen(s: Sent) {
  return s.stage === "pending" || s.stage === "included";
}

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
