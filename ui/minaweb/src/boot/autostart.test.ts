import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  carryHeld,
  takeHandoff,
  TAB_HANDOFF,
  type HandoffDb,
  type HandoffDeps,
  type HandoffEntry,
} from "../roles/handoff";
import type { HeldKey } from "../roles/keys";
import { restartWithKey } from "../roles/restart";
import { ROLE_KEY, VIEWER, type Role } from "../roles/role";
import { installStorage, MemoryStorage } from "../roles/test-storage";
import {
  AUTOSTART_KEY,
  autostartUrl,
  loadGate,
  newNonce,
  reloadAndStart,
  takeAutostart,
} from "./autostart";

// Shapes only: not real keys.
const PUBLIC = `B62q${"1".repeat(51)}`;
const key: HeldKey = {
  publicKey: PUBLIC,
  privateKey: `EK${"3".repeat(50)}`,
  origin: "generated",
};
const worker: Role = { kind: "worker", publicKey: PUBLIC, fee: "1" };
const producer: Role = { kind: "producer", publicKey: PUBLIC };
const PAGE = "https://minaweb.example/block-producer";

const entries = new Map<string, HandoffEntry>();
const db: HandoffDb = {
  put: async e => void entries.set(e.id, e),
  get: async id => entries.get(id) ?? null,
  take: async id => {
    const e = entries.get(id) ?? null;
    entries.delete(id);
    return e;
  },
  delete: async id => void entries.delete(id),
  list: async () => [...entries.values()].map(e => ({ id: e.id, at: e.at })),
};
const deps: HandoffDeps = { db, now: () => Date.now() };
const carry = (k: HeldKey) => carryHeld(k, deps);

/** The address bar of this tab. */
let address = PAGE;
/** The page's own navigation: to this address with the nonce. */
const go = vi.fn((nonce: string) => {
  address = autostartUrl(address, nonce);
});

/** A load of the page at the tab's address. */
async function load() {
  const held: HeldKey[] = [];
  const gate = loadGate(
    address,
    url => {
      address = new URL(url, address).href;
    },
    () => takeHandoff(deps),
    k => held.push(k),
  );
  const carried = await gate.carried;
  return { ...gate, carried, held };
}

/** Duplicating a tab copies its address and its sessionStorage. */
function duplicate() {
  const copy = new MemoryStorage();
  for (let i = 0; i < session.length; i++) {
    const name = session.key(i)!;
    copy.setItem(name, session.getItem(name)!);
  }
  installStorage("sessionStorage", copy);
  session = copy;
}

let session: MemoryStorage;
beforeEach(() => {
  entries.clear();
  address = PAGE;
  go.mockClear();
  session = new MemoryStorage();
  installStorage("sessionStorage", session);
  installStorage("localStorage", new MemoryStorage());
});

describe("a page-made navigation starts the node by itself", () => {
  for (const [name, role, held] of [
    ["SNARK worker start", worker, null],
    ["block producer start, its key carried", producer, key],
    ["role stop, with a key held", VIEWER, key],
    ["role stop, with no key", VIEWER, null],
  ] as const) {
    test(`after a ${name}`, async () => {
      await restartWithKey(role, held, null, go, carry);
      expect(go).toHaveBeenCalledOnce();
      expect(address).toMatch(/\?autostart=[0-9a-f]{32}$/);
      const after = await load();
      expect(after.autostart).toBe(true);
      expect(after.held).toEqual(held ? [key] : []);
      // The address is clean and the nonce gone.
      expect(address).toBe(PAGE);
      expect(session.getItem(AUTOSTART_KEY)).toBeNull();
      expect(session.getItem(TAB_HANDOFF)).toBeNull();
    });
  }

  test("after a producer's restart with its key under a passphrase", async () => {
    await restartWithKey(
      producer,
      key,
      { kind: "carry", passphrase: "correct horse battery" },
      go,
    );
    expect((await load()).autostart).toBe(true);
  }, 30_000);

  test("after the store's reset or a retry (reloadAndStart)", async () => {
    reloadAndStart(go);
    expect(go).toHaveBeenCalledOnce();
    expect((await load()).autostart).toBe(true);
  });

  test("keeping the path, the rest of the query and the hash", () => {
    const nonce = newNonce();
    session.setItem(AUTOSTART_KEY, nonce);
    const replace = vi.fn();
    const href = `${PAGE}?tab=b&autostart=${nonce}&x=1#top`;
    expect(takeAutostart(href, replace).autostart).toBe(true);
    expect(replace).toHaveBeenCalledWith("/block-producer?tab=b&x=1#top");
  });
});

describe("the start screen shows", () => {
  test("on a new visit", async () => {
    expect(await load()).toMatchObject({
      autostart: false,
      param: false,
      stored: false,
      carried: null,
    });
  });

  test("on a manual refresh after a page-made navigation: the address is clean", async () => {
    await restartWithKey(worker, null, null, go, carry);
    expect((await load()).autostart).toBe(true);
    // The worker's role stays in the tab; a refresh asks all the same.
    expect(await load()).toMatchObject({ autostart: false, param: false });
  });

  test("on a refresh after a page-made navigation that was stopped", async () => {
    // The nonce is stored; the navigation never happened: the old address.
    await restartWithKey(producer, key, null, () => {}, carry);
    const after = await load();
    expect(after).toMatchObject({
      autostart: false,
      param: false,
      stored: true,
    });
    // The stale nonce is gone, and the key is held before Start.
    expect(session.getItem(AUTOSTART_KEY)).toBeNull();
    expect(after.held).toEqual([key]);
    expect((await load()).autostart).toBe(false);
  });

  test("in a tab duplicated after a page-made navigation", async () => {
    await restartWithKey(worker, null, null, go, carry);
    expect((await load()).autostart).toBe(true);
    duplicate();
    expect((await load()).autostart).toBe(false);
  });

  test("in a tab duplicated from one whose navigation was stopped", async () => {
    await restartWithKey(worker, null, null, () => {}, carry);
    duplicate();
    expect(await load()).toMatchObject({ autostart: false, stored: true });
  });

  test("for an address with a nonce this tab didn't store", async () => {
    // Pasted, shared, bookmarked, or opened in another tab.
    address = autostartUrl(PAGE, newNonce());
    const after = await load();
    expect(after).toMatchObject({
      autostart: false,
      param: true,
      stored: false,
    });
    expect(address).toBe(PAGE);
  });

  test("for nonces that don't match, both taken", async () => {
    session.setItem(AUTOSTART_KEY, newNonce());
    address = autostartUrl(PAGE, newNonce());
    expect((await load()).autostart).toBe(false);
    expect(session.getItem(AUTOSTART_KEY)).toBeNull();
    expect(address).toBe(PAGE);
  });

  test("for a matching value that isn't a nonce", async () => {
    session.setItem(AUTOSTART_KEY, "");
    address = `${PAGE}?autostart=`;
    expect((await load()).autostart).toBe(false);
    session.setItem(AUTOSTART_KEY, "1");
    address = `${PAGE}?autostart=1`;
    expect((await load()).autostart).toBe(false);
  });

  test("if the address can't be cleaned", () => {
    const nonce = newNonce();
    session.setItem(AUTOSTART_KEY, nonce);
    const refuse = () => {
      throw new DOMException("refused", "SecurityError");
    };
    expect(takeAutostart(autostartUrl(PAGE, nonce), refuse).autostart).toBe(
      false,
    );
  });

  test("without sessionStorage", async () => {
    installStorage("sessionStorage", "denied");
    reloadAndStart(go);
    expect(go).toHaveBeenCalledOnce();
    expect((await load()).autostart).toBe(false);
  });

  test("for a carried key without a nonce, which is held all the same", async () => {
    await carry(key);
    const after = await load();
    expect(after.autostart).toBe(false);
    expect(after.held).toEqual([key]);
  });
});

describe("a role change refuses to navigate", () => {
  test("if storage won't keep the nonce", async () => {
    session.refuseWrites = true;
    await expect(
      restartWithKey(VIEWER, null, null, go, carry),
    ).rejects.toThrow();
    expect(go).not.toHaveBeenCalled();
    session.refuseWrites = false;
    expect(session.getItem(AUTOSTART_KEY)).toBeNull();
  });

  test("if storage won't keep the role, taking the nonce back", async () => {
    const discard = vi.fn(async () => {});
    const setItem = session.setItem.bind(session);
    vi.spyOn(session, "setItem").mockImplementation((name, value) => {
      if (name === ROLE_KEY) {
        throw new DOMException("full", "QuotaExceededError");
      }
      setItem(name, value);
    });
    await expect(
      restartWithKey(worker, key, null, go, carry, discard),
    ).rejects.toThrow();
    expect(go).not.toHaveBeenCalled();
    expect(discard).toHaveBeenCalledOnce();
    expect(session.getItem(AUTOSTART_KEY)).toBeNull();
  });
});

test("each nonce is new: 128 random bits", () => {
  const seen = new Set(Array.from({ length: 100 }, newNonce));
  expect(seen.size).toBe(100);
  for (const n of seen) expect(n).toMatch(/^[0-9a-f]{32}$/);
});
