import { beforeEach, describe, expect, test } from "vitest";

import {
  carryHeld,
  HANDOFF_MAX_AGE_MS,
  HandoffError,
  takeHandoff,
  TAB_HANDOFF,
  type HandoffDb,
  type HandoffDeps,
  type HandoffEntry,
} from "./handoff";
import type { HeldKey } from "./keys";
import { installStorage, MemoryStorage } from "./test-storage";

// Shapes only: not real keys.
const PUBLIC = `B62q${"1".repeat(51)}`;
const OTHER_PUBLIC = `B62q${"2".repeat(51)}`;
const PRIVATE = `EK${"3".repeat(50)}`;
const held: HeldKey = {
  publicKey: PUBLIC,
  privateKey: PRIVATE,
  origin: "imported",
};

/** IndexedDB as a Map: the values kept as they are, as structured clone
 * keeps a CryptoKey. */
class MemoryDb implements HandoffDb {
  map = new Map<string, HandoffEntry>();
  refusePut = false;
  loseOnGet = false;
  failTake = false;
  async put(entry: HandoffEntry) {
    if (this.refusePut) throw new DOMException("full", "QuotaExceededError");
    this.map.set(entry.id, entry);
  }
  async get(id: string) {
    return this.loseOnGet ? null : (this.map.get(id) ?? null);
  }
  async take(id: string) {
    if (this.failTake) throw new DOMException("aborted", "AbortError");
    const entry = this.loseOnGet ? null : (this.map.get(id) ?? null);
    this.map.delete(id);
    return entry;
  }
  async delete(id: string) {
    this.map.delete(id);
  }
  async list() {
    return [...this.map.values()].map(e => ({ id: e.id, at: e.at }));
  }
}

let session: MemoryStorage;
let db: MemoryDb;
let clock: number;
let deps: HandoffDeps;
beforeEach(() => {
  session = new MemoryStorage();
  installStorage("sessionStorage", session);
  installStorage("localStorage", new MemoryStorage());
  db = new MemoryDb();
  clock = 1_800_000_000_000;
  deps = { db, now: () => clock };
});

async function otherEntry(id: string, at: number): Promise<HandoffEntry> {
  const key = await crypto.subtle.generateKey(
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
  return { id, at, key };
}

describe("a held key across the reload", () => {
  test("comes back with no passphrase, and both stores are empty after", async () => {
    await carryHeld(held, deps);
    const text = session.getItem(TAB_HANDOFF)!;
    expect(text).not.toContain(PRIVATE);
    expect(JSON.parse(text).publicKey).toBe(PUBLIC);
    const [entry] = [...db.map.values()];
    expect(entry.key.extractable).toBe(false);
    expect(entry.key.algorithm.name).toBe("AES-GCM");

    clock += 3_000;
    expect(await takeHandoff(deps)).toEqual(held);
    expect(session.getItem(TAB_HANDOFF)).toBeNull();
    expect(db.map.size).toBe(0);
  });

  test("is taken once only", async () => {
    await carryHeld(held, deps);
    expect(await takeHandoff(deps)).toEqual(held);
    expect(await takeHandoff(deps)).toBeNull();
  });

  test("stale entries go at any load; another tab's fresh one stays", async () => {
    db.map.set("old", await otherEntry("old", clock - HANDOFF_MAX_AGE_MS - 1));
    db.map.set("other-tab", await otherEntry("other-tab", clock - 1_000));
    // A load with no record of its own.
    expect(await takeHandoff(deps)).toBeNull();
    expect([...db.map.keys()]).toEqual(["other-tab"]);

    await carryHeld(held, deps);
    expect(await takeHandoff(deps)).toEqual(held);
    expect([...db.map.keys()]).toEqual(["other-tab"]);

    // Once stale, a later load deletes it.
    clock += HANDOFF_MAX_AGE_MS + 1;
    expect(await takeHandoff(deps)).toBeNull();
    expect(db.map.size).toBe(0);
  });

  test("two tabs: a load in one leaves the other's handoff to it", async () => {
    // Tab A carries its key; tab B loads before A's reload.
    const tabA = session;
    await carryHeld(held, deps);
    installStorage("sessionStorage", new MemoryStorage());
    expect(await takeHandoff(deps)).toBeNull();
    // A's reload still finds its key.
    installStorage("sessionStorage", tabA);
    clock += 1_000;
    expect(await takeHandoff(deps)).toEqual(held);
    expect(db.map.size).toBe(0);
  });

  test("an entry made after this load read the clock stays", async () => {
    // Tab B reads the clock, then tab A's handoff lands before B's sweep.
    db.map.set("other-tab", await otherEntry("other-tab", clock + 1));
    db.map.set(
      "far-ahead",
      await otherEntry("far-ahead", clock + HANDOFF_MAX_AGE_MS + 1),
    );
    expect(await takeHandoff(deps)).toBeNull();
    expect([...db.map.keys()]).toEqual(["other-tab"]);
  });

  test("a take that fails restores nothing", async () => {
    await carryHeld(held, deps);
    db.failTake = true;
    expect(await takeHandoff(deps)).toBeNull();
    expect(session.getItem(TAB_HANDOFF)).toBeNull();
  });

  test("a record older than the limit isn't taken, and is deleted", async () => {
    await carryHeld(held, deps);
    clock += HANDOFF_MAX_AGE_MS + 1;
    expect(await takeHandoff(deps)).toBeNull();
    expect(session.getItem(TAB_HANDOFF)).toBeNull();
    expect(db.map.size).toBe(0);
  });

  test("a record changed in storage decrypts nothing, and is deleted", async () => {
    await carryHeld(held, deps);
    const record = JSON.parse(session.getItem(TAB_HANDOFF)!);
    session.setItem(
      TAB_HANDOFF,
      JSON.stringify({ ...record, publicKey: OTHER_PUBLIC }),
    );
    expect(await takeHandoff(deps)).toBeNull();
    expect(session.getItem(TAB_HANDOFF)).toBeNull();
    expect(db.map.size).toBe(0);
  });

  test("a record whose IndexedDB entry is gone: null, record deleted", async () => {
    await carryHeld(held, deps);
    db.map.clear();
    expect(await takeHandoff(deps)).toBeNull();
    expect(session.getItem(TAB_HANDOFF)).toBeNull();
  });
});

describe("where the browser can't carry it", () => {
  test("IndexedDB refuses the key: HandoffError, nothing left", async () => {
    db.refusePut = true;
    await expect(carryHeld(held, deps)).rejects.toBeInstanceOf(HandoffError);
    expect(session.getItem(TAB_HANDOFF)).toBeNull();
    expect(db.map.size).toBe(0);
  });

  test("IndexedDB loses the key: HandoffError, nothing left", async () => {
    db.loseOnGet = true;
    await expect(carryHeld(held, deps)).rejects.toBeInstanceOf(HandoffError);
    expect(session.getItem(TAB_HANDOFF)).toBeNull();
    expect(db.map.size).toBe(0);
  });

  test("sessionStorage refuses the copy: HandoffError, nothing left", async () => {
    session.refuseWrites = true;
    await expect(carryHeld(held, deps)).rejects.toBeInstanceOf(HandoffError);
    expect(db.map.size).toBe(0);
  });

  test("sessionStorage turned off: HandoffError, nothing left", async () => {
    installStorage("sessionStorage", "denied");
    await expect(carryHeld(held, deps)).rejects.toBeInstanceOf(HandoffError);
    expect(db.map.size).toBe(0);
  });

  test("no IndexedDB: HandoffError", async () => {
    await expect(
      carryHeld(held, { db: null, now: () => clock }),
    ).rejects.toBeInstanceOf(HandoffError);
    expect(session.getItem(TAB_HANDOFF)).toBeNull();
    expect(await takeHandoff({ db: null, now: () => clock })).toBeNull();
  });
});
