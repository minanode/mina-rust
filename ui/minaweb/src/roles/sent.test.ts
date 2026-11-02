import { describe, expect, test } from "vitest";

import type { Account, InclusionLook } from "./node";
import type { Sent } from "./payment";
import {
  parseSent,
  pruneSent,
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
    expect(after).toEqual(before.getSnapshot());
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

  test(`the list keeps the newest ${SENT_MAX}`, () => {
    const storage = new MemoryStorage();
    const s = store(storage);
    for (let n = 0; n < SENT_MAX + 5; n++) s.add(sent(n));
    expect(s.getSnapshot()).toHaveLength(SENT_MAX);
    expect(s.getSnapshot()[0].hash).toBe(hash(SENT_MAX + 4));
    expect(JSON.parse(storage.getItem(SENT_KEY)!)).toHaveLength(SENT_MAX);
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
