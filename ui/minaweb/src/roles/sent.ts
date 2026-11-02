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
 * the memo. A payment that is done is kept for SENT_KEEP_MS, and at most
 * SENT_MAX of them; one still open is never dropped, as its nonce is taken.
 *
 * The restarted node's pool is empty, so a payment kept from before the
 * restart doesn't hold its nonce until a look finds it in the pool or a
 * block. Until then the next nonce waits; if the look finds it in
 * neither, the next payment takes its nonce in its place (nonceFor).
 */

import { num, obj, str } from "@minanode/node-client";

import { looksLikePublicKey } from "./keys";
import type { Account, Inclusion, InclusionLook } from "./node";
import { nextStage, type Look, type Sent, type SentStage } from "./payment";

/** sessionStorage: this tab's sent payments. */
export const SENT_KEY = "minaweb.sent";
/** The most payments that are done the list keeps, newest first. */
export const SENT_MAX = 20;
/** The most open payments the tab has at once: a new one waits for a
 * block to take some. */
export const SENT_OPEN_MAX = 50;
/** Open payments the list keeps at most: SENT_OPEN_MAX holding nonces,
 * and the missing ones replacements took the place of. Past this, the
 * oldest of those go first; one that may still hold a nonce never does. */
export const SENT_OPEN_ROWS = 2 * SENT_OPEN_MAX;
/** A lookup that hasn't answered by then failed: a look never waits on a
 * busy node for good. */
export const LOOK_TIMEOUT_MS = 15_000;
/** Mina nonces are uint32. */
export const NONCE_MAX = 0xffff_ffff;
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

/** A missing payment whose nonce another open payment of this run holds:
 * the replacement took its place, so it holds nothing. It stays open to
 * be followed, as it may still land. */
function superseded(s: Sent, list: readonly Sent[]): boolean {
  return (
    s.check === "missing" &&
    list.some(
      o =>
        o !== s &&
        isOpen(o) &&
        o.check === undefined &&
        o.from === s.from &&
        o.nonce === s.nonce,
    )
  );
}

/** The list without what is done and old, and at most SENT_MAX of what
 * is done. Every open payment that may hold a nonce stays; past
 * SENT_OPEN_ROWS open, the oldest superseded ones go. */
export function pruneSent(list: readonly Sent[], now: number): Sent[] {
  let over = list.filter(isOpen).length - SENT_OPEN_ROWS;
  const drop = new Set<Sent>();
  for (let i = list.length - 1; i >= 0 && over > 0; i--) {
    if (superseded(list[i], list)) {
      drop.add(list[i]);
      over--;
    }
  }
  let done = 0;
  return list.filter(s => {
    if (drop.has(s)) return false;
    if (isOpen(s)) return true;
    if (now - s.sentAt > SENT_KEEP_MS || done >= SENT_MAX) return false;
    done++;
    return true;
  });
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

/** Null, or a string of at most DETAIL_MAX; undefined for anything else. */
const nullableText = (value: unknown): string | null | undefined =>
  value === null
    ? null
    : typeof value === "string" && value.length <= DETAIL_MAX
      ? value
      : undefined;

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
  const failure = nullableText(i?.failure);
  if (
    height === null ||
    depth === null ||
    blockHash === null ||
    !HASH.test(blockHash) ||
    failure === undefined
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
  const nonceOk = nonce !== null && nonce <= NONCE_MAX;
  const sentAt = count(s?.sentAt);
  const stage = STAGES.find(x => x === s?.stage);
  const detail = nullableText(s?.detail);
  const inclusion = parseInclusion(s?.inclusion);
  if (
    hash === null ||
    !HASH.test(hash) ||
    from === null ||
    to === null ||
    amount === null ||
    fee === null ||
    nonce === null ||
    !nonceOk ||
    sentAt === null ||
    stage === undefined ||
    detail === undefined ||
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
  for (const item of value.slice(0, SENT_MAX + SENT_OPEN_ROWS)) {
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
    // The restarted node's pool doesn't have them: unchecked until a look.
    this.list = pruneSent(parseSent(raw), now()).map(s =>
      isOpen(s) ? { ...s, check: "unchecked" } : s,
    );
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
  /** A new payment, first; one with the same hash is replaced, unless it
   * is open and the new one isn't: a refusal of the same payment sent
   * again never frees the nonce it holds. */
  add(sent: Sent) {
    const held = this.list.find(s => s.hash === sent.hash);
    if (held && isOpen(held) && !isOpen(sent)) return;
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
/** The lookup's answer, or undefined if it fails or takes past `ms`. */
function bounded<T>(lookup: () => Promise<T>, ms: number) {
  return new Promise<T | undefined>(resolve => {
    const timer = setTimeout(() => resolve(undefined), ms);
    const done = (value: T | undefined) => {
      clearTimeout(timer);
      resolve(value);
    };
    try {
      lookup().then(done, () => done(undefined));
    } catch {
      done(undefined);
    }
  });
}

export async function trackOnce(
  node: TrackNode,
  store: SentStore,
  depth = SEARCH_DEPTH,
  timeoutMs = LOOK_TIMEOUT_MS,
): Promise<void> {
  const open = store.getSnapshot().filter(isOpen);
  const nonces = new Map<string, Promise<number | null | undefined>>();
  const nonceOf = (from: string) => {
    let n = nonces.get(from);
    if (!n) {
      n = bounded(
        () => node.account(from).then(a => (a ? a.nonce : null)),
        timeoutMs,
      );
      nonces.set(from, n);
    }
    return n;
  };
  await Promise.all(
    open.map(async s => {
      const [inclusion, inPool, accountNonce] = await Promise.all([
        bounded(() => node.inclusion(s.hash, depth), timeoutMs),
        bounded(() => node.inPool(s.hash), timeoutMs),
        nonceOf(s.from),
      ]);
      const look = { inclusion, inPool, accountNonce };
      store.update(s.hash, current => {
        const next = nextStage(current, look);
        return { ...next, check: nextCheck(current, look) };
      });
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

/** A kept payment's check after a look: found in the pool or a block, it
 * holds its nonce again; a look that answers everything and finds it in
 * neither makes it missing. A lookup that failed makes an unchecked one
 * failed, and changes nothing else: time never frees a nonce. */
export function nextCheck(sent: Sent, look: Look): Sent["check"] {
  if (sent.check === undefined) return undefined;
  if (look.inclusion?.found || look.inPool === true) return undefined;
  if (
    look.inclusion === undefined ||
    look.inPool === undefined ||
    look.accountNonce === undefined
  ) {
    return sent.check === "missing" ? "missing" : "failed";
  }
  // No best tip yet: nothing to go on.
  if (look.inclusion === null) return sent.check;
  return "missing";
}

export type NonceState =
  /** The next payment's nonce. */
  | { kind: "ready"; nonce: number }
  /** A payment kept from before the restart has this nonce, and no look
   * has answered for it yet; `failed` once a look failed. */
  | { kind: "checking"; nonce: number; failed: boolean }
  /** A payment kept from before the restart has this nonce, and is in
   * neither the pool nor a block: the next payment takes its place. */
  | { kind: "missing"; nonce: number; payment: Sent }
  /** SENT_OPEN_MAX open payments hold nonces. */
  | { kind: "full" }
  /** No uint32 nonce is left. */
  | { kind: "exhausted" };

/**
 * The nonce for a new payment from this account: the account's, past the
 * run of this tab's open payments from it that hold the next nonce each.
 * A payment after a gap holds nothing: the gap's nonce comes first.
 */
export function nonceFor(account: Account, sent: readonly Sent[]): NonceState {
  const open = sent.filter(s => s.from === account.publicKey && isOpen(s));
  let nonce = account.nonce;
  for (;;) {
    if (nonce > NONCE_MAX) return { kind: "exhausted" };
    const at = open.filter(s => s.nonce === nonce);
    if (at.length === 0) break;
    if (!at.some(s => s.check === undefined)) {
      if (at.some(s => s.check === "unchecked")) {
        return { kind: "checking", nonce, failed: false };
      }
      if (at.some(s => s.check === "failed")) {
        return { kind: "checking", nonce, failed: true };
      }
      // A replacement takes the missing payment's place, not a new one:
      // the cap below doesn't apply.
      return { kind: "missing", nonce, payment: at[0] };
    }
    nonce++;
  }
  const holding = sent.filter(s => isOpen(s) && !superseded(s, sent));
  if (holding.length >= SENT_OPEN_MAX) return { kind: "full" };
  return { kind: "ready", nonce };
}
