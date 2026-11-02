/**
 * The tab's key across a role change's reload, with no passphrase. Each
 * restart makes a fresh AES-256-GCM key in WebCrypto, non-extractable, and
 * keeps it in this origin's IndexedDB: the page can use it, but not read it
 * out. The private key, encrypted with it, goes in this tab's
 * sessionStorage with a random id, the public key and the time. The next
 * load of the tab decrypts the key, holds it and deletes both. Every load
 * also deletes IndexedDB entries older than the limit; another tab's fresh
 * entry is left for that tab. An entry a crash or a closed tab leaves stays
 * until a later load of the site, and an entry alone decrypts nothing. No
 * passphrase and no plain private key is stored.
 *
 * Where the browser has no IndexedDB or WebCrypto, or either refuses a
 * write, `carryHeld` throws HandoffError and the page falls back to the
 * passphrase (restart.ts).
 */

import {
  looksLikePrivateKey,
  looksLikePublicKey,
  removeStored,
  writeVerified,
  type HeldKey,
} from "./keys";

/** sessionStorage: the encrypted key, for this tab's next load only. */
export const TAB_HANDOFF = "minaweb.key-handoff";
/** IndexedDB: the AES keys, by handoff id. */
export const HANDOFF_DB = "minaweb-key-handoff";
const STORE = "keys";
/** Older than this, an entry is deleted and a record not taken. */
export const HANDOFF_MAX_AGE_MS = 2 * 60_000;

export interface HandoffRecord {
  version: 1;
  id: string;
  publicKey: string;
  origin: HeldKey["origin"];
  at: number;
  iv: string;
  ciphertext: string;
}

export interface HandoffEntry {
  id: string;
  at: number;
  key: CryptoKey;
}

/** The IndexedDB store, behind an interface the tests replace. */
export interface HandoffDb {
  put(entry: HandoffEntry): Promise<void>;
  get(id: string): Promise<HandoffEntry | null>;
  /** Reads and deletes in one transaction; rejects if either fails. */
  take(id: string): Promise<HandoffEntry | null>;
  delete(id: string): Promise<void>;
  list(): Promise<{ id: string; at: number }[]>;
}

/** The handoff isn't possible here, or failed: nothing was reloaded. */
export class HandoffError extends Error {}

const HANDOFF_FAILED =
  "This browser couldn't keep the key across the restart by itself. Nothing restarted.";

function request<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error ?? new Error("IndexedDB request failed"));
  });
}

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open(HANDOFF_DB, 1);
    r.onupgradeneeded = () => {
      r.result.createObjectStore(STORE, { keyPath: "id" });
    };
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error ?? new Error("IndexedDB open failed"));
    r.onblocked = () => reject(new Error("IndexedDB open blocked"));
  });
}

async function withStore<T>(
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  const db = await openDb();
  try {
    const tx = db.transaction(STORE, mode);
    const result = await request(run(tx.objectStore(STORE)));
    await new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error ?? new Error("IndexedDB failed"));
      tx.onabort = () => reject(tx.error ?? new Error("IndexedDB aborted"));
    });
    return result;
  } finally {
    db.close();
  }
}

export const indexedDbHandoff: HandoffDb = {
  async put(entry) {
    await withStore("readwrite", s => s.put(entry));
  },
  async get(id) {
    const entry = (await withStore("readonly", s => s.get(id))) as
      HandoffEntry | undefined;
    return entry ?? null;
  },
  async take(id) {
    const entry = (await withStore("readwrite", s => {
      const r = s.get(id);
      r.addEventListener("success", () => {
        if (r.result !== undefined) s.delete(id);
      });
      return r;
    })) as HandoffEntry | undefined;
    return entry ?? null;
  },
  async delete(id) {
    await withStore("readwrite", s => s.delete(id));
  },
  async list() {
    const all = (await withStore("readonly", s =>
      s.getAll(),
    )) as HandoffEntry[];
    return all.map(e => ({ id: String(e?.id), at: Number(e?.at) }));
  },
};

export interface HandoffDeps {
  /** Null where the browser has no IndexedDB. */
  db: HandoffDb | null;
  now: () => number;
}

function defaultDeps(): HandoffDeps {
  let db: HandoffDb | null = null;
  try {
    if (typeof indexedDB !== "undefined" && indexedDB) db = indexedDbHandoff;
  } catch {
    // IndexedDB turned off.
  }
  return { db, now: Date.now };
}

/** WebCrypto and IndexedDB are there. A write can still be refused. */
export function handoffSupported(deps: HandoffDeps = defaultDeps()): boolean {
  return (
    deps.db !== null &&
    typeof crypto !== "undefined" &&
    typeof crypto.subtle?.generateKey === "function"
  );
}

const encoder = new TextEncoder();

function toBase64(bytes: Uint8Array): string {
  let text = "";
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text);
}

function fromBase64(text: string): Uint8Array<ArrayBuffer> {
  const raw = atob(text);
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  return bytes;
}

/** Every field but the ciphertext, bound to it as additional data. */
function additionalData(r: Omit<HandoffRecord, "iv" | "ciphertext">) {
  return encoder.encode(
    JSON.stringify([r.version, r.id, r.publicKey, r.origin, r.at]),
  );
}

export function parseHandoff(text: string | null): HandoffRecord | null {
  if (!text) return null;
  try {
    const r = JSON.parse(text) as Partial<HandoffRecord>;
    if (
      r.version === 1 &&
      typeof r.id === "string" &&
      r.id.length > 0 &&
      typeof r.publicKey === "string" &&
      looksLikePublicKey(r.publicKey) &&
      (r.origin === "generated" || r.origin === "imported") &&
      typeof r.at === "number" &&
      typeof r.iv === "string" &&
      typeof r.ciphertext === "string"
    ) {
      return r as HandoffRecord;
    }
  } catch {
    // Not a record of ours.
  }
  return null;
}

function readTabRecord(): HandoffRecord | null {
  try {
    return parseHandoff(sessionStorage.getItem(TAB_HANDOFF));
  } catch {
    return null;
  }
}

function isCryptoKey(value: unknown): value is CryptoKey {
  return typeof CryptoKey !== "undefined" && value instanceof CryptoKey;
}

/**
 * Keeps `key` for this tab's next load. Resolves only once the AES key is
 * in IndexedDB, decrypts the copy when read back, and the copy is in
 * sessionStorage, exactly. Otherwise it deletes what it wrote and throws
 * HandoffError.
 */
export async function carryHeld(
  key: HeldKey,
  deps: HandoffDeps = defaultDeps(),
): Promise<void> {
  const db = deps.db;
  if (!db || !handoffSupported(deps)) throw new HandoffError(HANDOFF_FAILED);
  const id = toBase64(crypto.getRandomValues(new Uint8Array(16)));
  try {
    const aes = await crypto.subtle.generateKey(
      { name: "AES-GCM", length: 256 },
      false,
      ["encrypt", "decrypt"],
    );
    if (aes.extractable) throw new Error("extractable key");
    const head = {
      version: 1 as const,
      id,
      publicKey: key.publicKey,
      origin: key.origin,
      at: deps.now(),
    };
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const ciphertext = await crypto.subtle.encrypt(
      { name: "AES-GCM", iv, additionalData: additionalData(head) },
      aes,
      encoder.encode(key.privateKey),
    );
    const record: HandoffRecord = {
      ...head,
      iv: toBase64(iv),
      ciphertext: toBase64(new Uint8Array(ciphertext)),
    };
    await db.put({ id, at: head.at, key: aes });
    // What the next load will do: read the key back and decrypt with it.
    const back = await db.get(id);
    if (
      !back ||
      back.at !== head.at ||
      !isCryptoKey(back.key) ||
      back.key.extractable
    ) {
      throw new Error("the stored key didn't come back");
    }
    const plain = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv, additionalData: additionalData(head) },
      back.key,
      ciphertext,
    );
    if (new TextDecoder().decode(plain) !== key.privateKey) {
      throw new Error("the stored key doesn't decrypt the copy");
    }
    writeVerified("session", TAB_HANDOFF, JSON.stringify(record));
  } catch {
    removeStored("session", TAB_HANDOFF);
    await db.delete(id).catch(() => {});
    throw new HandoffError(HANDOFF_FAILED);
  }
}

/** Deletes a carried key that won't be used: the tab didn't reload. */
export async function discardHandoff(deps: HandoffDeps = defaultDeps()) {
  const record = readTabRecord();
  removeStored("session", TAB_HANDOFF);
  if (record) await deps.db?.delete(record.id).catch(() => {});
}

/**
 * At each load, before anything else: the key the last load of this tab
 * carried, or null. Both copies are deleted, as far as storage allows,
 * whether or not the key comes back, and so is every expired IndexedDB
 * entry. Another tab's fresh entry stays, even one made after this load
 * read the clock: that tab may be reloading now.
 */
export async function takeHandoff(
  deps: HandoffDeps = defaultDeps(),
): Promise<HeldKey | null> {
  const record = readTabRecord();
  removeStored("session", TAB_HANDOFF);
  const db = deps.db;
  if (!db || !handoffSupported(deps)) return null;
  const now = deps.now();
  let entry: HandoffEntry | null = null;
  if (record) entry = await db.take(record.id).catch(() => null);
  // One a crash or a closed tab left behind.
  try {
    for (const e of await db.list()) {
      // Expired, or impossibly far ahead (the clock moved back).
      if (
        !Number.isFinite(e.at) ||
        now - e.at > HANDOFF_MAX_AGE_MS ||
        e.at - now > HANDOFF_MAX_AGE_MS
      ) {
        await db.delete(e.id).catch(() => {});
      }
    }
  } catch {
    // Nothing to sweep in a store that won't open.
  }
  if (
    !record ||
    !entry ||
    entry.at !== record.at ||
    !isCryptoKey(entry.key) ||
    !(now - record.at >= 0 && now - record.at <= HANDOFF_MAX_AGE_MS)
  ) {
    return null;
  }
  try {
    const plain = await crypto.subtle.decrypt(
      {
        name: "AES-GCM",
        iv: fromBase64(record.iv),
        additionalData: additionalData(record),
      },
      entry.key,
      fromBase64(record.ciphertext),
    );
    const privateKey = new TextDecoder().decode(plain);
    if (!looksLikePrivateKey(privateKey)) return null;
    return { publicKey: record.publicKey, privateKey, origin: record.origin };
  } catch {
    return null;
  }
}
