import { describe, expect, test } from "vitest";

import {
  decryptKey,
  encryptKey,
  exportKeyFile,
  importKey,
  importKeyFile,
  KeyStore,
  MAX_ITERATIONS,
  parseEncryptedKey,
  SAVED_KEY,
  StorageError,
  type HeldKey,
  type KeyFileGlue,
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

  test("a record asking for too many iterations is refused, saying so", async () => {
    const record = await encryptKey(key, "correct horse battery", FAST);
    const at = (iterations: number) => ({ ...record, iterations });
    expect(MAX_ITERATIONS).toBe(1_200_000);
    // Read from storage, so the visitor sees it and can delete it.
    expect(parseEncryptedKey(JSON.stringify(at(2 ** 31)))?.iterations).toBe(
      2 ** 31,
    );
    expect(parseEncryptedKey(JSON.stringify(at(600_000.5)))).toBeNull();
    // Refused before any derivation, so it returns at once.
    for (const iterations of [MAX_ITERATIONS + 1, 2 ** 31]) {
      await expect(
        decryptKey(at(iterations), "correct horse battery", glue),
      ).rejects.toThrow(
        "This saved key asks for more PBKDF2 iterations than the page allows.",
      );
    }
    // What the page writes still unlocks.
    const written = await encryptKey(key, "correct horse battery");
    expect(written.iterations).toBe(600_000);
    expect(
      (await decryptKey(written, "correct horse battery", glue)).privateKey,
    ).toBe(PRIVATE);
  });

  test("an unlocked key that doesn't derive its public key is refused", async () => {
    const other: HeldKey = { ...key, privateKey: `EK${"4".repeat(50)}` };
    const record = await encryptKey(other, "correct horse battery", FAST);
    await expect(
      decryptKey(record, "correct horse battery", glue),
    ).rejects.toThrow("The saved key doesn't match its public key.");
  });
});

describe("a key carried across a reload", () => {
  test("is kept if it derives its public key, dropped if not", () => {
    const store = new KeyStore();
    store.hold(key);
    store.dropIfMismatched(glue);
    expect(store.getSnapshot().key).toEqual(key);

    // The right shape, but the glue derives another public key from it,
    // or none.
    for (const privateKey of [`EK${"4".repeat(50)}`, PRIVATE]) {
      const mismatched: HeldKey = {
        ...key,
        privateKey,
        publicKey: privateKey === PRIVATE ? OTHER : PUBLIC,
      };
      store.hold(mismatched);
      store.dropIfMismatched(glue);
      expect(store.getSnapshot().key).toBeNull();
    }
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

describe("a key file", () => {
  const FILE = '{"box_primitive":"xsalsa20poly1305"}';
  const fileGlue = (answer: unknown): KeyFileGlue => ({
    account_key_from_file: async () => answer,
    account_key_to_file: async () => FILE,
  });

  test("gives the WASM's key, as an imported one", async () => {
    const pair = { publicKey: PUBLIC, privateKey: PRIVATE };
    expect(
      await importKeyFile(fileGlue(pair), `\n${FILE}\n`, "password"),
    ).toEqual({ ...pair, origin: "imported" });
  });

  test("refuses an answer that isn't a key pair, and an empty password", async () => {
    for (const answer of [null, {}, { publicKey: PUBLIC }, "text"]) {
      await expect(
        importKeyFile(fileGlue(answer), FILE, "password"),
      ).rejects.toThrow("The page couldn't read that key file.");
    }
    await expect(
      importKeyFile(
        fileGlue({ publicKey: PUBLIC, privateKey: PRIVATE }),
        FILE,
        "",
      ),
    ).rejects.toThrow("Type the key file's password.");
  });

  test("exports under a password of ten characters", async () => {
    await expect(exportKeyFile(fileGlue(null), key, "short")).rejects.toThrow(
      /10/,
    );
    expect(await exportKeyFile(fileGlue(null), key, "long enough!")).toBe(FILE);
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
