import { describe, expect, test } from "vitest";

import {
  decryptKey,
  encryptKey,
  importKey,
  KeyStore,
  parseEncryptedKey,
  SAVED_KEY,
  StorageError,
  type HeldKey,
  type KeyGlue,
} from "./keys";
import { installStorage, MemoryStorage } from "./test-storage";

// Shapes only: the glue below is a stand-in, these are not real keys.
const PUBLIC = `B62q${"1".repeat(51)}`;
const OTHER = `B62q${"2".repeat(51)}`;
const PRIVATE = `EK${"3".repeat(50)}`;
const key: HeldKey = {
  publicKey: PUBLIC,
  privateKey: PRIVATE,
  origin: "generated",
};

const glue: KeyGlue = {
  generate_account_key: () => ({ publicKey: PUBLIC, privateKey: PRIVATE }),
  account_public_key: k => {
    if (k !== PRIVATE) throw new Error(`bad key ${k}`);
    return PUBLIC;
  },
  account_public_key_valid: k => k === PUBLIC || k === OTHER,
};

// Few iterations: the tests check the format, not the cost.
const FAST = 1_000;

describe("an encrypted key", () => {
  test("decrypts with its passphrase, to the same key", async () => {
    const record = await encryptKey(key, "correct horse battery", FAST);
    expect(record.publicKey).toBe(PUBLIC);
    expect(JSON.stringify(record)).not.toContain(PRIVATE);
    const back = await decryptKey(record, "correct horse battery", glue);
    expect(back.privateKey).toBe(PRIVATE);
    expect(back.publicKey).toBe(PUBLIC);
  });

  test("refuses another passphrase, saying only that", async () => {
    const record = await encryptKey(key, "correct horse battery", FAST);
    await expect(decryptKey(record, "wrong passphrase!")).rejects.toThrow(
      /^Wrong passphrase\.$/,
    );
  });

  test("is bound to its public key", async () => {
    const record = await encryptKey(key, "correct horse battery", FAST);
    const moved = { ...record, publicKey: OTHER };
    await expect(decryptKey(moved, "correct horse battery")).rejects.toThrow(
      "Wrong passphrase.",
    );
  });

  test("needs a passphrase of ten characters", async () => {
    await expect(encryptKey(key, "short", FAST)).rejects.toThrow(/10/);
  });

  test("a stored record is read only if it is one of ours", async () => {
    const record = await encryptKey(key, "correct horse battery", FAST);
    // Too few iterations to trust from storage.
    expect(parseEncryptedKey(JSON.stringify(record))).toBeNull();
    const strong = { ...record, iterations: 600_000 };
    expect(parseEncryptedKey(JSON.stringify(strong))?.publicKey).toBe(PUBLIC);
    expect(parseEncryptedKey("not json")).toBeNull();
    expect(parseEncryptedKey(JSON.stringify({ version: 2 }))).toBeNull();
  });
});

describe("importing a key", () => {
  test("takes a private key and finds its public key", () => {
    expect(importKey(glue, `  ${PRIVATE}\n`)).toMatchObject({
      publicKey: PUBLIC,
      origin: "imported",
    });
  });

  test("its errors never repeat what was pasted", () => {
    const pasted = [`EK${"4".repeat(50)}`, PUBLIC, "EKnot-a-key", "hello"];
    for (const text of pasted) {
      let message = "";
      try {
        importKey(glue, text);
      } catch (e) {
        message = (e as Error).message;
      }
      expect(message).not.toBe("");
      expect(message).not.toContain(text);
    }
  });
});

describe("saving the key in this browser", () => {
  test("says so only once the record is there", async () => {
    const local = new MemoryStorage();
    installStorage("localStorage", local);
    const store = new KeyStore();
    store.hold(key);
    await store.remember("correct horse battery");
    expect(store.getSnapshot().saved?.publicKey).toBe(PUBLIC);
    expect(local.getItem(SAVED_KEY)).toContain("PBKDF2-SHA-256");
    expect(local.getItem(SAVED_KEY)).not.toContain(PRIVATE);
    // A fresh store, as after a reload, finds it.
    expect(new KeyStore().getSnapshot().saved?.publicKey).toBe(PUBLIC);
  });

  test("storage turned off: the save fails, and says why", async () => {
    installStorage("localStorage", "denied");
    const store = new KeyStore();
    store.hold(key);
    await expect(store.remember("correct horse battery")).rejects.toThrow(
      StorageError,
    );
    await expect(store.remember("correct horse battery")).rejects.toThrow(
      /nothing was saved/,
    );
    expect(store.getSnapshot().saved).toBeNull();
  });

  test("a full quota: the same", async () => {
    const local = new MemoryStorage();
    local.refuseWrites = true;
    installStorage("localStorage", local);
    const store = new KeyStore();
    store.hold(key);
    await expect(store.remember("correct horse battery")).rejects.toThrow(
      StorageError,
    );
    expect(store.getSnapshot().saved).toBeNull();
    expect(new KeyStore().getSnapshot().saved).toBeNull();
  });
});
