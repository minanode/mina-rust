import { describe, expect, test, vi } from "vitest";

import type { Account, InclusionLook } from "./node";
import type { Sent } from "./payment";
import {
  nextCheck,
  NONCE_MAX,
  nonceFor,
  parseSent,
  pruneSent,
  LOOK_TIMEOUT_MS,
  SENT_OPEN_MAX,
  SENT_OPEN_ROWS,
  SENT_KEEP_MS,
  SENT_KEY,
  SENT_MAX,
  SentStore,
  Tracker,
  trackOnce,
  type TrackNode,
} from "./sent";
import { MemoryStorage } from "./test-storage";

const FROM = `B62q${"1".repeat(51)}`;
const TO = `B62q${"2".repeat(51)}`;

const payment = (hash: string, nonce: number): Sent => ({
  hash,
  from: FROM,
  to: TO,
  amount: 1n,
  fee: 1n,
  nonce,
  memo: "",
  // Recent: a payment that is done and old leaves the list.
  sentAt: Date.now(),
  stage: "pending",
  detail: null,
  inclusion: null,
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(r => (resolve = r));
  return { promise, resolve };
}

const account = (nonce: number): Account => ({
  publicKey: FROM,
  balance: 10n,
  nonce,
  delegate: null,
});

describe("following sent payments", () => {
  test("a payment accepted while a look is out survives it", async () => {
    const store = new SentStore();
    store.add(payment("A", 0));
    const look = deferred<InclusionLook | null>();
    const node: TrackNode = {
      account: async () => account(0),
      inclusion: () => look.promise,
      inPool: async () => true,
    };
    const tracking = trackOnce(node, store);
    // Accepted meanwhile, as the send form does.
    store.add(payment("B", 1));
    look.resolve({
      tipHeight: 100,
      depthSearched: 40,
      found: { height: 99, blockHash: "3NA", depth: 1, failure: null },
    });
    await tracking;
    const list = store.getSnapshot();
    expect(list.map(s => s.hash)).toEqual(["B", "A"]);
    expect(list.find(s => s.hash === "A")?.stage).toBe("included");
    expect(list.find(s => s.hash === "B")?.stage).toBe("pending");
  });

  test("looks don't overlap: a tick while one is out is skipped", async () => {
    const store = new SentStore();
    store.add(payment("A", 0));
    const look = deferred<InclusionLook | null>();
    let asked = 0;
    const node: TrackNode = {
      account: async () => account(0),
      inclusion: () => {
        asked++;
        return look.promise;
      },
      inPool: async () => true,
    };
    const tracker = new Tracker(node, store);
    const first = tracker.tick();
    expect(await tracker.tick()).toBe(false);
    look.resolve(null);
    expect(await first).toBe(true);
    expect(asked).toBe(1);
  });

  test("a block that leaves the best chain sends it back to the pool", async () => {
    const store = new SentStore();
    store.add(payment("A", 0));
    let found: InclusionLook = {
      tipHeight: 100,
      depthSearched: 40,
      found: { height: 100, blockHash: "3NA", depth: 0, failure: null },
    };
    const node: TrackNode = {
      account: async () => account(0),
      inclusion: async () => found,
      inPool: async () => true,
    };
    await trackOnce(node, store);
    expect(store.getSnapshot()[0].stage).toBe("included");
    // A reorg: the tip moved one block, the search still reaches height
    // 100, and the command isn't there.
    found = { tipHeight: 101, depthSearched: 40, found: null };
    await trackOnce(node, store);
    const [a] = store.getSnapshot();
    expect(a.stage).toBe("pending");
    expect(a.inclusion).toBeNull();
    expect(a.detail).toMatch(/left the best chain.*back in the pool/);
  });

  test("a block older than the search keeps its place", async () => {
    const store = new SentStore();
    store.add({
      ...payment("A", 0),
      stage: "included",
      inclusion: { height: 50, blockHash: "3NA", depth: 10, failure: null },
    });
    const node: TrackNode = {
      account: async () => account(1),
      inclusion: async () => ({
        tipHeight: 100,
        depthSearched: 40,
        found: null,
      }),
      inPool: async () => false,
    };
    await trackOnce(node, store);
    expect(store.getSnapshot()[0]).toMatchObject({
      stage: "included",
      inclusion: { height: 50 },
    });
  });

  test("a lookup that fails is unknown, not absent", async () => {
    const store = new SentStore();
    store.add({
      ...payment("A", 0),
      stage: "included",
      inclusion: { height: 99, blockHash: "3NA", depth: 1, failure: null },
    });
    const node: TrackNode = {
      account: async () => {
        throw new Error("busy");
      },
      inclusion: async () => {
        throw new Error("busy");
      },
      inPool: async () => {
        throw new Error("busy");
      },
    };
    await trackOnce(node, store);
    expect(store.getSnapshot()[0]).toMatchObject({
      stage: "included",
      inclusion: { height: 99 },
    });
  });

  test("each sender's nonce is its own", async () => {
    const store = new SentStore();
    const other = `B62q${"3".repeat(51)}`;
    store.add({ ...payment("A", 0), from: other });
    const asked: string[] = [];
    const node: TrackNode = {
      account: async pk => {
        asked.push(pk);
        return { ...account(5), publicKey: pk };
      },
      inclusion: async () => ({
        tipHeight: 100,
        depthSearched: 40,
        found: null,
      }),
      inPool: async () => false,
    };
    await trackOnce(node, store);
    expect(asked).toEqual([other]);
  });
});

test("one poll reads each sender once and never applies another sender's nonce", async () => {
  const store = new SentStore();
  store.add({ ...payment("old-4", 4), from: "old" });
  store.add({ ...payment("fresh-0", 0), from: "fresh" });
  store.add({ ...payment("fresh-1", 1), from: "fresh" });
  const asked: string[] = [];
  await trackOnce(
    {
      account: async publicKey => {
        asked.push(publicKey);
        return { ...account(publicKey === "old" ? 5 : 0), publicKey };
      },
      inclusion: async () => ({
        tipHeight: 100,
        depthSearched: 40,
        found: null,
      }),
      inPool: async () => false,
    },
    store,
  );
  expect(asked.sort()).toEqual(["fresh", "old"]);
  expect(store.getSnapshot().find(s => s.hash === "old-4")?.detail).toMatch(
    /nonce moved/,
  );
  expect(
    store
      .getSnapshot()
      .filter(s => s.from === "fresh")
      .every(s => !s.detail?.includes("nonce moved")),
  ).toBe(true);
});

describe("keeping the list across a restart", () => {
  const NOW = 1_793_643_210_000;
  const HOUR = 60 * 60_000;
  const hash = (n: number) => `5Ju${String(n).padStart(4, "0")}`;
  const sent = (n: number, change: Partial<Sent> = {}): Sent => ({
    ...payment(hash(n), n),
    amount: 1_000_000_000n,
    fee: 100_000_000n,
    sentAt: NOW,
    ...change,
  });
  const store = (storage: Storage | null, now = NOW) =>
    new SentStore(
      () => storage,
      () => now,
    );

  test("a new store in the same tab has the list, as it was", () => {
    const storage = new MemoryStorage();
    const before = store(storage);
    before.add(sent(0));
    before.add(sent(1));
    before.update(hash(0), () => ({
      stage: "included",
      inclusion: { height: 583746, blockHash: "3NLr", depth: 2, failure: null },
    }));

    const after = store(storage, NOW + 60_000).getSnapshot();
    // The same, and each open payment unchecked until a look.
    expect(after).toEqual(
      before.getSnapshot().map(s => ({ ...s, check: "unchecked" })),
    );
    expect(after.map(s => s.hash)).toEqual([hash(1), hash(0)]);
    expect(after[1].amount).toBe(1_000_000_000n);
    expect(after[1].inclusion?.height).toBe(583746);
  });

  test("storage holds the public fields only: no memo, nothing else", () => {
    const storage = new MemoryStorage();
    const s = store(storage);
    s.add({ ...sent(0), memo: "a note", secret: "EKxyz" } as Sent);
    const raw = storage.getItem(SENT_KEY)!;
    expect(raw).not.toContain("a note");
    expect(raw).not.toContain("EK");
    expect(Object.keys(JSON.parse(raw)[0]).sort()).toEqual([
      "amount",
      "detail",
      "fee",
      "from",
      "hash",
      "inclusion",
      "nonce",
      "sentAt",
      "stage",
      "to",
    ]);
    expect(store(storage).getSnapshot()[0].memo).toBe("");
  });

  test(`the list keeps the newest ${SENT_MAX} done, and every open one`, () => {
    const storage = new MemoryStorage();
    const s = store(storage);
    s.add(sent(100));
    for (let n = 0; n < SENT_MAX + 5; n++) {
      s.add(sent(n, { stage: "confirmed" }));
    }
    const list = s.getSnapshot();
    expect(list).toHaveLength(SENT_MAX + 1);
    expect(list[0].hash).toBe(hash(SENT_MAX + 4));
    expect(list.at(-1)?.hash).toBe(hash(100));
    expect(JSON.parse(storage.getItem(SENT_KEY)!)).toHaveLength(SENT_MAX + 1);
  });

  test("a payment that is done and old leaves; an open one stays", () => {
    const old = NOW - SENT_KEEP_MS - HOUR;
    const list = [
      sent(0, { stage: "confirmed", sentAt: old }),
      sent(1, { stage: "dropped", sentAt: old }),
      sent(2, { stage: "included", sentAt: old }),
      sent(3, { stage: "pending", sentAt: old }),
      sent(4, { stage: "confirmed", sentAt: NOW - HOUR }),
    ];
    expect(pruneSent(list, NOW).map(s => s.nonce)).toEqual([2, 3, 4]);

    // On load too, and storage drops them.
    const storage = new MemoryStorage();
    const s = store(storage, old);
    for (const p of [...list].reverse()) s.add(p);
    expect(s.getSnapshot()).toHaveLength(5);
    expect(
      store(storage)
        .getSnapshot()
        .map(s => s.nonce),
    ).toEqual([2, 3, 4]);
    expect(JSON.parse(storage.getItem(SENT_KEY)!)).toHaveLength(3);
  });

  test("the last payment leaving removes the entry", () => {
    const storage = new MemoryStorage();
    store(storage, NOW - SENT_KEEP_MS - HOUR).add(
      sent(0, { stage: "confirmed", sentAt: NOW - SENT_KEEP_MS - HOUR }),
    );
    expect(storage.getItem(SENT_KEY)).not.toBeNull();
    expect(store(storage).getSnapshot()).toEqual([]);
    expect(storage.getItem(SENT_KEY)).toBeNull();
  });

  test("stored values that don't parse are left out", () => {
    const good = JSON.parse(
      (() => {
        const storage = new MemoryStorage();
        store(storage).add(sent(0));
        return storage.getItem(SENT_KEY)!;
      })(),
    )[0];
    expect(parseSent(null)).toEqual([]);
    expect(parseSent("not json")).toEqual([]);
    expect(parseSent('{"hash":"x"}')).toEqual([]);
    const list = parseSent(
      JSON.stringify([
        good,
        good,
        { ...good, hash: hash(1), amount: 1 },
        { ...good, hash: hash(2), amount: "-1" },
        { ...good, hash: hash(3), to: ` ${TO}` },
        { ...good, hash: hash(4), stage: "spent" },
        { ...good, hash: hash(5), nonce: 1.5 },
        { ...good, hash: hash(6), inclusion: { height: 1 } },
        { ...good, hash: "not a hash!" },
        { ...good, hash: hash(7), detail: "x".repeat(1000) },
        null,
        { ...good, hash: hash(8) },
      ]),
    );
    expect(list.map(s => s.hash)).toEqual([hash(0), hash(8)]);
  });

  test("a tab without storage, or with it full, keeps the list in memory", () => {
    const full = new MemoryStorage();
    full.refuseWrites = true;
    const s = store(full);
    s.add(sent(0));
    expect(s.getSnapshot()).toHaveLength(1);
    expect(full.getItem(SENT_KEY)).toBeNull();

    const denied = new SentStore(() => {
      throw new DOMException("The operation is insecure.", "SecurityError");
    });
    denied.add(sent(0));
    expect(denied.getSnapshot()).toHaveLength(1);
  });
});

describe("the next nonce", () => {
  const ledger = (nonce: number): Account => ({ ...account(nonce) });
  const at = (nonce: number, change: Partial<Sent> = {}): Sent => ({
    ...payment(`5Ju${nonce}`, nonce),
    ...change,
  });

  test("it follows this tab's open payments from the account, in a run", () => {
    expect(nonceFor(ledger(4), [])).toEqual({ kind: "ready", nonce: 4 });
    expect(nonceFor(ledger(4), [at(4), at(5)])).toEqual({
      kind: "ready",
      nonce: 6,
    });
    expect(nonceFor(ledger(4), [at(4, { stage: "rejected" })])).toEqual({
      kind: "ready",
      nonce: 4,
    });
    // Another key's payments don't move this one's nonce.
    expect(nonceFor({ ...ledger(0), publicKey: TO }, [at(4), at(5)])).toEqual({
      kind: "ready",
      nonce: 0,
    });
    // A payment after a gap holds nothing: the gap comes first.
    expect(nonceFor(ledger(4), [at(6)])).toEqual({ kind: "ready", nonce: 4 });
    expect(nonceFor(ledger(4), [at(4), at(6)])).toEqual({
      kind: "ready",
      nonce: 5,
    });
  });

  test("a kept payment waits for a look, then gives up its nonce if missing", () => {
    expect(nonceFor(ledger(7), [at(7, { check: "unchecked" })])).toEqual({
      kind: "checking",
      nonce: 7,
      failed: false,
    });
    expect(nonceFor(ledger(7), [at(7, { check: "failed" })])).toEqual({
      kind: "checking",
      nonce: 7,
      failed: true,
    });
    const missing = at(7, { check: "missing" });
    expect(nonceFor(ledger(7), [missing])).toEqual({
      kind: "missing",
      nonce: 7,
      payment: missing,
    });
    // Its replacement holds 7.
    expect(nonceFor(ledger(7), [at(7), missing])).toEqual({
      kind: "ready",
      nonce: 8,
    });
    // Below the account's nonce it doesn't matter.
    expect(nonceFor(ledger(8), [missing])).toEqual({ kind: "ready", nonce: 8 });
  });

  test("no nonce past uint32, and a cap on open payments", () => {
    expect(nonceFor(ledger(NONCE_MAX), [])).toEqual({
      kind: "ready",
      nonce: NONCE_MAX,
    });
    expect(nonceFor(ledger(NONCE_MAX), [at(NONCE_MAX)])).toEqual({
      kind: "exhausted",
    });
    const open = Array.from({ length: SENT_OPEN_MAX }, (_, n) => at(n + 100));
    expect(nonceFor(ledger(0), open)).toEqual({ kind: "full" });
  });

  test("a look decides a kept payment's check", () => {
    const kept = at(7, { check: "unchecked" });
    const none = { tipHeight: 100, depthSearched: 40, found: null };
    const found = {
      ...none,
      found: { height: 99, blockHash: "3NA", depth: 1, failure: null },
    };
    const check = (look: Partial<Parameters<typeof nextCheck>[1]>) =>
      nextCheck(kept, {
        inclusion: none,
        inPool: false,
        accountNonce: 7,
        ...look,
      });
    expect(check({})).toBe("missing");
    expect(check({ inPool: true })).toBeUndefined();
    expect(check({ inclusion: found })).toBeUndefined();
    // A lookup that failed: failed, never missing. No best tip: unchanged.
    expect(check({ inPool: undefined })).toBe("failed");
    expect(check({ inclusion: undefined })).toBe("failed");
    expect(check({ accountNonce: undefined })).toBe("failed");
    expect(check({ inclusion: null })).toBe("unchecked");
    // A missing one stays missing when a later lookup fails.
    expect(
      nextCheck(at(7, { check: "missing" }), {
        inclusion: undefined,
        inPool: false,
        accountNonce: 7,
      }),
    ).toBe("missing");
    // A payment this run's node had is never checked.
    expect(
      nextCheck(at(7), { inclusion: none, inPool: false, accountNonce: 7 }),
    ).toBeUndefined();
  });
});

describe("review findings", () => {
  const FUNDED: Account = { ...account(7) };
  const kept: Sent = {
    ...payment("5JuReview", 7),
    sentAt: Date.now(),
  };

  test("a kept payment absent from the restarted pool and chain gives up its nonce", async () => {
    const storage = new MemoryStorage();
    new SentStore(() => storage).add(kept);
    const restored = new SentStore(() => storage);
    expect(restored.getSnapshot()[0].check).toBe("unchecked");
    expect(nonceFor(FUNDED, restored.getSnapshot()).kind).toBe("checking");
    await trackOnce(
      {
        account: async () => FUNDED,
        inPool: async () => false,
        inclusion: async () => ({
          found: null,
          tipHeight: 100,
          depthSearched: 40,
        }),
      },
      restored,
    );
    expect(restored.getSnapshot()[0].check).toBe("missing");
    expect(nonceFor(FUNDED, restored.getSnapshot())).toMatchObject({
      kind: "missing",
      nonce: 7,
    });
  });

  test("a kept payment the restarted pool has holds its nonce", async () => {
    const storage = new MemoryStorage();
    new SentStore(() => storage).add(kept);
    const restored = new SentStore(() => storage);
    await trackOnce(
      {
        account: async () => FUNDED,
        inPool: async () => true,
        inclusion: async () => ({
          found: null,
          tipHeight: 100,
          depthSearched: 40,
        }),
      },
      restored,
    );
    expect(restored.getSnapshot()[0].check).toBeUndefined();
    expect(nonceFor(FUNDED, restored.getSnapshot())).toEqual({
      kind: "ready",
      nonce: 8,
    });
    // `check` is never stored: the next restart checks again.
    expect(storage.getItem(SENT_KEY)).not.toContain("check");
  });

  test("a nonce past uint32 doesn't parse", () => {
    const storage = new MemoryStorage();
    new SentStore(() => storage).add(kept);
    const raw = JSON.parse(storage.getItem(SENT_KEY)!);
    for (const nonce of [Number.MAX_SAFE_INTEGER, NONCE_MAX + 1, -1, 1.5]) {
      raw[0].nonce = nonce;
      expect(parseSent(JSON.stringify(raw))).toEqual([]);
    }
    raw[0].nonce = NONCE_MAX;
    expect(parseSent(JSON.stringify(raw))).toHaveLength(1);
  });

  test("nullable text is null or a string, nothing else", () => {
    const storage = new MemoryStorage();
    new SentStore(() => storage).add(kept);
    const good = JSON.parse(storage.getItem(SENT_KEY)!)[0];
    const inclusion = { height: 1, depth: 1, blockHash: "3NLr", failure: null };
    const parses = (change: object) =>
      parseSent(JSON.stringify([{ ...good, ...change }])).length === 1;
    expect(parses({ detail: null })).toBe(true);
    expect(parses({ detail: "Looking for it again." })).toBe(true);
    expect(parses({ detail: { bad: true } })).toBe(false);
    expect(parses({ detail: 42 })).toBe(false);
    expect(parses({ detail: undefined })).toBe(false);
    expect(parses({ inclusion })).toBe(true);
    expect(parses({ inclusion: { ...inclusion, failure: "Overflow" } })).toBe(
      true,
    );
    expect(parses({ inclusion: { ...inclusion, failure: 42 } })).toBe(false);
    expect(parses({ inclusion: { ...inclusion, failure: undefined } })).toBe(
      false,
    );
  });

  test("refused attempts don't push out an open payment", () => {
    const store = new SentStore();
    store.add(kept);
    for (let n = 0; n < SENT_MAX + 5; n++) {
      store.add({
        ...kept,
        hash: `5JuRefused${n}`,
        nonce: 8,
        stage: "rejected",
      });
    }
    const list = store.getSnapshot();
    expect(list.filter(s => s.stage === "rejected")).toHaveLength(SENT_MAX);
    expect(list.some(s => s.hash === kept.hash)).toBe(true);
    expect(nonceFor(FUNDED, list)).toEqual({ kind: "ready", nonce: 8 });
  });
});

describe("re-review findings", () => {
  const FUNDED: Account = { ...account(7) };
  const none = async () => ({ found: null, tipHeight: 100, depthSearched: 40 });
  const kept = (n: number): Sent => ({
    ...payment(`5JuKept${n}`, 7 + n),
    sentAt: Date.now(),
  });
  const restart = (list: Sent[]) => {
    const storage = new MemoryStorage();
    const before = new SentStore(() => storage);
    for (const s of [...list].reverse()) before.add(s);
    return new SentStore(() => storage);
  };

  test("50 missing payments can each be replaced, without a new slot", async () => {
    const store = restart(
      Array.from({ length: SENT_OPEN_MAX }, (_, n) => kept(n)),
    );
    await trackOnce(
      {
        account: async () => FUNDED,
        inPool: async () => false,
        inclusion: none,
      },
      store,
    );
    expect(store.getSnapshot().every(s => s.check === "missing")).toBe(true);
    expect(nonceFor(FUNDED, store.getSnapshot())).toMatchObject({
      kind: "missing",
      nonce: 7,
    });
    // Replace each in turn: every one is offered, none is full.
    for (let n = 0; n < SENT_OPEN_MAX; n++) {
      const state = nonceFor(FUNDED, store.getSnapshot());
      expect(state).toMatchObject({ kind: "missing", nonce: 7 + n });
      store.add({ ...kept(n), hash: `5JuNew${n}` });
    }
    // All 50 nonces held by replacements: a new nonce waits for blocks.
    expect(nonceFor(FUNDED, store.getSnapshot())).toEqual({ kind: "full" });
    expect(store.getSnapshot()).toHaveLength(2 * SENT_OPEN_MAX);
  });

  test("a refusal of an open payment's hash never replaces it", () => {
    const store = new SentStore();
    store.add(kept(0));
    store.add({ ...kept(0), stage: "rejected", detail: "Duplicate" });
    expect(store.getSnapshot()).toHaveLength(1);
    expect(store.getSnapshot()[0].stage).toBe("pending");
    expect(nonceFor(FUNDED, store.getSnapshot())).toEqual({
      kind: "ready",
      nonce: 8,
    });
    // A done one is replaced, as before.
    store.update(kept(0).hash, () => ({ stage: "dropped" }));
    store.add({ ...kept(0), stage: "rejected" });
    expect(store.getSnapshot()[0].stage).toBe("rejected");
  });

  test("failed lookups keep the nonce; a look that answers decides it", async () => {
    const store = restart([kept(0)]);
    const fail = async () => {
      throw new Error("busy");
    };
    for (let n = 0; n < 3; n++) {
      await trackOnce(
        { account: async () => FUNDED, inPool: fail, inclusion: none },
        store,
      );
    }
    expect(nonceFor(FUNDED, store.getSnapshot())).toEqual({
      kind: "checking",
      nonce: 7,
      failed: true,
    });
    await trackOnce(
      {
        account: async () => FUNDED,
        inPool: async () => true,
        inclusion: none,
      },
      store,
    );
    expect(nonceFor(FUNDED, store.getSnapshot())).toEqual({
      kind: "ready",
      nonce: 8,
    });
  });

  test("a lookup that never answers times out as failed", async () => {
    const store = restart([kept(0)]);
    const tracker = new Tracker(
      {
        account: async () => FUNDED,
        inPool: () => new Promise(() => {}),
        inclusion: none,
      },
      store,
    );
    vi.useFakeTimers();
    try {
      const tick = tracker.tick();
      await vi.advanceTimersByTimeAsync(LOOK_TIMEOUT_MS);
      expect(await tick).toBe(true);
      expect(store.getSnapshot()[0].check).toBe("failed");
      // The tracker is free for the next look.
      const next = tracker.tick();
      await vi.advanceTimersByTimeAsync(LOOK_TIMEOUT_MS);
      expect(await next).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  test("superseded missing payments go first past the row cap", () => {
    const list: Sent[] = [];
    for (let n = 0; n < SENT_OPEN_ROWS + 5; n++) {
      list.push({ ...kept(n), nonce: 7 + (n % 60), hash: `5JuRow${n}` });
    }
    // The 5 oldest rows: missing, their nonces held by newer rows.
    for (let n = SENT_OPEN_ROWS; n < SENT_OPEN_ROWS + 5; n++) {
      list[n] = { ...list[n], check: "missing" };
    }
    const pruned = pruneSent(list, Date.now());
    expect(pruned).toHaveLength(SENT_OPEN_ROWS);
    expect(pruned.some(s => s.check === "missing")).toBe(false);
    // One that may still hold a nonce is never dropped.
    const unheld = [...list];
    unheld[SENT_OPEN_ROWS] = { ...unheld[SENT_OPEN_ROWS], nonce: 1000 };
    expect(pruneSent(unheld, Date.now()).map(s => s.nonce)).toContain(1000);
  });
});
