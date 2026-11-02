import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  carryHeld,
  HandoffError,
  takeHandoff,
  TAB_HANDOFF,
  type HandoffDb,
  type HandoffDeps,
  type HandoffEntry,
} from "./handoff";
import {
  decryptKey,
  encryptKey,
  HANDOFF_KEY,
  readStored,
  type HeldKey,
} from "./keys";
import { keyAtRisk, restartWithKey } from "./restart";
import { readRole, ROLE_KEY, runRoleArgs, VIEWER, type Role } from "./role";
import { installStorage, MemoryStorage } from "./test-storage";

// Shapes only: not real keys.
const PUBLIC = `B62q${"1".repeat(51)}`;
const PRIVATE = `EK${"3".repeat(50)}`;
const generated: HeldKey = {
  publicKey: PUBLIC,
  privateKey: PRIVATE,
  origin: "generated",
};
const worker: Role = { kind: "worker", publicKey: PUBLIC, fee: "1" };
const producer: Role = { kind: "producer", publicKey: PUBLIC };
const RECEIVER = `B62q${"2".repeat(51)}`;
const paying: Role = { ...producer, coinbaseReceiver: RECEIVER };
const PASS = "correct horse battery";

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
const deps: HandoffDeps = { db, now: () => 1_800_000_000_000 };
const carry = (key: HeldKey) => carryHeld(key, deps);
const refuse = async () => {
  throw new HandoffError("This browser couldn't keep the key.");
};

let session: MemoryStorage;
beforeEach(() => {
  entries.clear();
  session = new MemoryStorage();
  installStorage("sessionStorage", session);
  installStorage("localStorage", new MemoryStorage());
});

/** After the reload, as the boot takes it: no passphrase. */
const afterReload = () => takeHandoff(deps);

/** After the reload, from the passphrase fallback. */
async function afterReloadWithPassphrase(): Promise<HeldKey | null> {
  const record = readStored("session", HANDOFF_KEY);
  return record ? decryptKey(record, PASS) : null;
}

describe("a role change with a key held in the tab", () => {
  for (const [name, role] of [
    ["to the worker", worker],
    ["to a viewer", VIEWER],
    ["to the producer", producer],
    ["to the producer, paying another key", paying],
  ] as const) {
    test(`${name}: no passphrase, and the key is held after`, async () => {
      const reload = vi.fn();
      await restartWithKey(role, generated, null, reload, carry);
      expect(reload).toHaveBeenCalledOnce();
      expect(readRole("devnet")).toEqual(role);
      expect(session.getItem(TAB_HANDOFF)).not.toContain(PRIVATE);
      expect(session.getItem(HANDOFF_KEY)).toBeNull();
      expect(await afterReload()).toEqual(generated);
      expect(session.getItem(TAB_HANDOFF)).toBeNull();
      expect(entries.size).toBe(0);
    });
  }

  test("the browser can't carry it: HandoffError, no reload, no role", async () => {
    const reload = vi.fn();
    await expect(
      restartWithKey(worker, generated, null, reload, refuse),
    ).rejects.toBeInstanceOf(HandoffError);
    expect(reload).not.toHaveBeenCalled();
    expect(session.getItem(ROLE_KEY)).toBeNull();
  });

  test("storage refuses the role: the carried key is deleted, no reload", async () => {
    const reload = vi.fn();
    const discard = vi.fn(async () => {});
    const writes = session.setItem.bind(session);
    session.setItem = (k, v) => {
      if (k === ROLE_KEY) throw new DOMException("full", "QuotaExceededError");
      writes(k, v);
    };
    await expect(
      restartWithKey(worker, generated, null, reload, carry, discard),
    ).rejects.toThrow(/storage/);
    expect(discard).toHaveBeenCalledOnce();
    expect(reload).not.toHaveBeenCalled();
  });
});

test("a producer's coinbase receiver reaches run after the reload", async () => {
  await restartWithKey(paying, generated, null, vi.fn(), carry);
  const role = readRole("devnet");
  const key = await afterReload();
  expect(runRoleArgs(role, key)).toEqual({
    blockProducer: PRIVATE,
    coinbaseReceiver: RECEIVER,
    snarker: null,
    snarkerFee: null,
  });
});

describe("the passphrase fallback", () => {
  test("carried: the same key comes back", async () => {
    const reload = vi.fn();
    await restartWithKey(
      worker,
      generated,
      { kind: "carry", passphrase: PASS },
      reload,
      refuse,
    );
    expect(reload).toHaveBeenCalledOnce();
    expect(session.getItem(HANDOFF_KEY)).not.toContain(PRIVATE);
    expect((await afterReloadWithPassphrase())?.privateKey).toBe(PRIVATE);
  });

  test("dropped only when the visitor says so", async () => {
    const reload = vi.fn();
    await restartWithKey(VIEWER, generated, { kind: "drop" }, reload, refuse);
    expect(reload).toHaveBeenCalledOnce();
    expect(await afterReloadWithPassphrase()).toBeNull();
  });

  test("storage that refuses the copy: no reload", async () => {
    session.refuseWrites = true;
    const reload = vi.fn();
    await expect(
      restartWithKey(
        worker,
        generated,
        { kind: "carry", passphrase: PASS },
        reload,
        refuse,
      ),
    ).rejects.toThrow(/storage/);
    expect(reload).not.toHaveBeenCalled();
  });

  test("the producer can't drop its key", async () => {
    const reload = vi.fn();
    await expect(
      restartWithKey(producer, generated, { kind: "drop" }, reload, refuse),
    ).rejects.toThrow(/needs its key/);
    await restartWithKey(
      producer,
      generated,
      { kind: "carry", passphrase: PASS },
      reload,
      refuse,
    );
    expect((await afterReloadWithPassphrase())?.publicKey).toBe(PUBLIC);
    expect(readRole("devnet")).toEqual(producer);
  });

  test("only a key not saved here is at risk", async () => {
    const saved = await encryptKey(generated, PASS, 1_000);
    expect(keyAtRisk(generated, saved)).toBe(false);
    expect(keyAtRisk(generated, null)).toBe(true);
    expect(keyAtRisk(null, null)).toBe(false);
  });
});

describe("no key", () => {
  test("nothing to carry", async () => {
    const reload = vi.fn();
    const spy = vi.fn(carry);
    await restartWithKey(VIEWER, null, null, reload, spy);
    expect(spy).not.toHaveBeenCalled();
    expect(reload).toHaveBeenCalledOnce();
  });

  test("storage turned off: no reload either", async () => {
    installStorage("sessionStorage", "denied");
    const reload = vi.fn();
    await expect(restartWithKey(worker, null, null, reload)).rejects.toThrow(
      /storage/,
    );
    expect(reload).not.toHaveBeenCalled();
  });

  test("the producer refuses a key that isn't the role's", async () => {
    await expect(restartWithKey(producer, null, null, vi.fn())).rejects.toThrow(
      /isn't the one/,
    );
  });
});
