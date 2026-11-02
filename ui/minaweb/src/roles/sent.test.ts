import { describe, expect, test } from "vitest";

import type { Account, InclusionLook } from "./node";
import type { Sent } from "./payment";
import { SentStore, Tracker, trackOnce, type TrackNode } from "./sent";

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
  sentAt: 0,
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
