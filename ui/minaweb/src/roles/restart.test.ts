import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  decryptKey,
  encryptKey,
  HANDOFF_KEY,
  readStored,
  type HeldKey,
} from "./keys";
import { KEY_CHOICE_NEEDED, keyAtRisk, restartWithKey } from "./restart";
import { readRole, ROLE_KEY, VIEWER, type Role } from "./role";
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
const PASS = "correct horse battery";

let session: MemoryStorage;
beforeEach(() => {
  session = new MemoryStorage();
  installStorage("sessionStorage", session);
  installStorage("localStorage", new MemoryStorage());
});

/** After the reload: what a new tab state could get back. */
async function afterReload(): Promise<HeldKey | null> {
  const record = readStored("session", HANDOFF_KEY);
  return record ? decryptKey(record, PASS) : null;
}

describe("a role change with a key that isn't saved", () => {
  // viewer to worker, producer to worker, producer to viewer: the page's
  // current role doesn't matter, only the key it holds.
  for (const [name, role] of [
    ["to the worker", worker],
    ["to a viewer", VIEWER],
  ] as const) {
    test(`${name}: refused without a choice, nothing reloads`, async () => {
      const reload = vi.fn();
      await expect(
        restartWithKey(role, generated, null, null, reload),
      ).rejects.toThrow(KEY_CHOICE_NEEDED);
      expect(reload).not.toHaveBeenCalled();
      expect(session.getItem(ROLE_KEY)).toBeNull();
    });

    test(`${name}: carried, the same key comes back`, async () => {
      const reload = vi.fn();
      await restartWithKey(
        role,
        generated,
        null,
        { kind: "carry", passphrase: PASS },
        reload,
      );
      expect(reload).toHaveBeenCalledOnce();
      expect(session.getItem(HANDOFF_KEY)).not.toContain(PRIVATE);
      expect((await afterReload())?.privateKey).toBe(PRIVATE);
      expect(readRole("devnet")).toEqual(role);
    });

    test(`${name}: dropped only when the visitor says so`, async () => {
      const reload = vi.fn();
      await restartWithKey(role, generated, null, { kind: "drop" }, reload);
      expect(reload).toHaveBeenCalledOnce();
      expect(await afterReload()).toBeNull();
    });
  }

  test("storage that refuses the copy: no reload", async () => {
    session.refuseWrites = true;
    const reload = vi.fn();
    await expect(
      restartWithKey(
        worker,
        generated,
        null,
        { kind: "carry", passphrase: PASS },
        reload,
      ),
    ).rejects.toThrow(/storage/);
    expect(reload).not.toHaveBeenCalled();
  });

  test("storage turned off: no reload either, even without a key", async () => {
    installStorage("sessionStorage", "denied");
    const reload = vi.fn();
    await expect(
      restartWithKey(worker, null, null, null, reload),
    ).rejects.toThrow(/storage/);
    expect(reload).not.toHaveBeenCalled();
  });
});

describe("keys that a reload doesn't lose", () => {
  test("a key saved in this browser needs no choice", async () => {
    const saved = await encryptKey(generated, PASS, 1_000);
    expect(keyAtRisk(generated, saved)).toBe(false);
    const reload = vi.fn();
    await restartWithKey(worker, generated, saved, null, reload);
    expect(reload).toHaveBeenCalledOnce();
  });

  test("no key, nothing to lose", async () => {
    expect(keyAtRisk(null, null)).toBe(false);
    const reload = vi.fn();
    await restartWithKey(VIEWER, null, null, null, reload);
    expect(reload).toHaveBeenCalledOnce();
  });
});

describe("the producer's start", () => {
  test("always carries its key, even one saved here", async () => {
    const saved = await encryptKey(generated, PASS, 1_000);
    const reload = vi.fn();
    await expect(
      restartWithKey(producer, generated, saved, { kind: "drop" }, reload),
    ).rejects.toThrow(/needs its key/);
    await restartWithKey(
      producer,
      generated,
      saved,
      { kind: "carry", passphrase: PASS },
      reload,
    );
    expect((await afterReload())?.publicKey).toBe(PUBLIC);
    expect(readRole("devnet")).toEqual(producer);
  });

  test("refuses a key that isn't the role's", async () => {
    await expect(
      restartWithKey(producer, null, null, null, vi.fn()),
    ).rejects.toThrow(/isn't the one/);
  });
});
