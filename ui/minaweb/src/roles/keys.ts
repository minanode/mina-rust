/**
 * The page's account key: made in this browser or imported, held in this
 * tab's memory only. It is written to storage only encrypted with a
 * passphrase the visitor chooses: in this browser's localStorage if they
 * ask the page to remember it, and in this tab's sessionStorage for the one
 * restart that starts the block producer. Nothing here logs it, puts it in
 * a URL or sends it anywhere: it goes to the node's signer and to `run`
 * only, in this page.
 *
 * The encryption is WebCrypto's: PBKDF2-SHA-256 (600,000 iterations, a
 * random 16-byte salt) derives an AES-256-GCM key, with a random 12-byte IV,
 * and the public key as additional data, so a record can't be passed off as
 * another key's.
 */

/** The parts of the WASM glue the key screen uses (crates/node/web/src/keys.rs). */
export interface KeyGlue {
  generate_account_key(): unknown;
  account_public_key(privateKey: string): string;
  account_public_key_valid(publicKey: string): boolean;
}

export interface HeldKey {
  publicKey: string;
  privateKey: string;
  origin: "generated" | "imported";
}

export interface EncryptedKey {
  version: 1;
  publicKey: string;
  kdf: "PBKDF2-SHA-256";
  iterations: number;
  salt: string;
  iv: string;
  ciphertext: string;
}

export const ITERATIONS = 600_000;
export const MIN_PASSPHRASE = 10;
/** localStorage: the key the visitor asked this browser to remember. */
export const SAVED_KEY = "minaweb.key";
/** sessionStorage: the producer's key across its restart, then deleted. */
export const HANDOFF_KEY = "minaweb.producer-key";

const PUBLIC_KEY = /^B62[1-9A-HJ-NP-Za-km-z]{52}$/;
const PRIVATE_KEY = /^EK[1-9A-HJ-NP-Za-km-z]{50}$/;

export function looksLikePublicKey(value: string): boolean {
  return PUBLIC_KEY.test(value.trim());
}

/** A typed key's shape, before the glue checks its checksum. */
export function looksLikePrivateKey(value: string): boolean {
  return PRIVATE_KEY.test(value.trim());
}

/** A new key, from the browser's CSPRNG in the WASM. */
export function generateKey(glue: KeyGlue): HeldKey {
  const pair = glue.generate_account_key() as {
    publicKey?: unknown;
    privateKey?: unknown;
  } | null;
  if (
    !pair ||
    typeof pair.publicKey !== "string" ||
    typeof pair.privateKey !== "string" ||
    !looksLikePublicKey(pair.publicKey) ||
    !looksLikePrivateKey(pair.privateKey)
  ) {
    throw new Error("The node made no key.");
  }
  return {
    publicKey: pair.publicKey,
    privateKey: pair.privateKey,
    origin: "generated",
  };
}

/** A pasted private key, checked and with its public key. The error never
 * repeats what was pasted. */
export function importKey(glue: KeyGlue, text: string): HeldKey {
  const privateKey = text.trim();
  if (looksLikePublicKey(privateKey)) {
    throw new Error(
      "That is a public key (B62…). Importing takes the private key (EK…).",
    );
  }
  if (!looksLikePrivateKey(privateKey)) {
    throw new Error(
      "That isn't a Mina private key: it starts EK, 52 characters.",
    );
  }
  let publicKey: string;
  try {
    publicKey = glue.account_public_key(privateKey);
  } catch {
    throw new Error("That isn't a valid Mina private key: its checksum fails.");
  }
  return { publicKey, privateKey, origin: "imported" };
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

async function deriveKey(
  passphrase: string,
  salt: Uint8Array<ArrayBuffer>,
  iterations: number,
): Promise<CryptoKey> {
  const material = await crypto.subtle.importKey(
    "raw",
    encoder.encode(passphrase),
    "PBKDF2",
    false,
    ["deriveKey"],
  );
  return crypto.subtle.deriveKey(
    { name: "PBKDF2", hash: "SHA-256", salt, iterations },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
}

export function passphraseProblem(passphrase: string): string | null {
  return passphrase.length < MIN_PASSPHRASE
    ? `Use at least ${MIN_PASSPHRASE} characters.`
    : null;
}

export async function encryptKey(
  key: HeldKey,
  passphrase: string,
  iterations = ITERATIONS,
): Promise<EncryptedKey> {
  const problem = passphraseProblem(passphrase);
  if (problem) throw new Error(problem);
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const aes = await deriveKey(passphrase, salt, iterations);
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv, additionalData: encoder.encode(key.publicKey) },
    aes,
    encoder.encode(key.privateKey),
  );
  return {
    version: 1,
    publicKey: key.publicKey,
    kdf: "PBKDF2-SHA-256",
    iterations,
    salt: toBase64(salt),
    iv: toBase64(iv),
    ciphertext: toBase64(new Uint8Array(ciphertext)),
  };
}

/** The key in a record, or an error that says only that the passphrase
 * is wrong (or the record damaged). */
export async function decryptKey(
  record: EncryptedKey,
  passphrase: string,
  glue?: Pick<KeyGlue, "account_public_key">,
): Promise<HeldKey> {
  let privateKey: string;
  try {
    const aes = await deriveKey(
      passphrase,
      fromBase64(record.salt),
      record.iterations,
    );
    const plain = await crypto.subtle.decrypt(
      {
        name: "AES-GCM",
        iv: fromBase64(record.iv),
        additionalData: encoder.encode(record.publicKey),
      },
      aes,
      fromBase64(record.ciphertext),
    );
    privateKey = new TextDecoder().decode(plain);
  } catch {
    throw new Error("Wrong passphrase.");
  }
  if (glue && glue.account_public_key(privateKey) !== record.publicKey) {
    throw new Error("The saved key doesn't match its public key.");
  }
  return { publicKey: record.publicKey, privateKey, origin: "imported" };
}

export function parseEncryptedKey(text: string | null): EncryptedKey | null {
  if (!text) return null;
  try {
    const r = JSON.parse(text) as Partial<EncryptedKey>;
    if (
      r.version === 1 &&
      r.kdf === "PBKDF2-SHA-256" &&
      typeof r.publicKey === "string" &&
      looksLikePublicKey(r.publicKey) &&
      typeof r.iterations === "number" &&
      r.iterations >= 100_000 &&
      typeof r.salt === "string" &&
      typeof r.iv === "string" &&
      typeof r.ciphertext === "string"
    ) {
      return r as EncryptedKey;
    }
  } catch {
    // Not a record of ours.
  }
  return null;
}

/** Storage that refused a write, or isn't there (private browsing, a
 * full quota, storage turned off). */
export class StorageError extends Error {}

const STORAGE_UNAVAILABLE =
  "This browser's storage is unavailable (private browsing, full, or turned off), so nothing was saved.";

function storage(kind: "local" | "session"): Storage {
  try {
    const s = kind === "local" ? localStorage : sessionStorage;
    if (!s) throw new Error("no storage");
    return s;
  } catch {
    throw new StorageError(STORAGE_UNAVAILABLE);
  }
}

export function readStored(
  kind: "local" | "session",
  name: string,
): EncryptedKey | null {
  try {
    return parseEncryptedKey(storage(kind).getItem(name));
  } catch {
    return null;
  }
}

/** Writes a value and reads it back; throws StorageError unless it is
 * there afterwards, exactly. */
export function writeVerified(
  kind: "local" | "session",
  name: string,
  text: string,
) {
  const s = storage(kind);
  try {
    s.setItem(name, text);
  } catch {
    throw new StorageError(STORAGE_UNAVAILABLE);
  }
  let back: string | null = null;
  try {
    back = s.getItem(name);
  } catch {
    // Read below as missing.
  }
  if (back !== text) throw new StorageError(STORAGE_UNAVAILABLE);
}

export function writeStored(
  kind: "local" | "session",
  name: string,
  record: EncryptedKey,
) {
  writeVerified(kind, name, JSON.stringify(record));
}

export function removeStored(kind: "local" | "session", name: string) {
  try {
    storage(kind).removeItem(name);
  } catch {
    // Nothing to remove from storage that isn't there.
  }
}

/**
 * The key this tab holds, for React (useSyncExternalStore). One per page,
 * as there is one node per page.
 */
export class KeyStore {
  private key: HeldKey | null = null;
  private saved: EncryptedKey | null = readStored("local", SAVED_KEY);
  private listeners = new Set<() => void>();
  private snapshot = { key: this.key, saved: this.saved };

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = () => this.snapshot;

  private changed() {
    this.snapshot = { key: this.key, saved: this.saved };
    for (const listener of this.listeners) listener();
  }

  hold(key: HeldKey) {
    this.key = key;
    this.changed();
  }

  /** Drops the key from this tab (not from storage). Clearing a reference
   * doesn't erase the string from memory: closing the tab does. */
  forget() {
    this.key = null;
    this.changed();
  }

  /** Saves the key encrypted in localStorage. Throws, and changes
   * nothing, unless the record is there afterwards. */
  async remember(passphrase: string) {
    if (!this.key) throw new Error("No key to save.");
    const record = await encryptKey(this.key, passphrase);
    writeStored("local", SAVED_KEY, record);
    if (readStored("local", SAVED_KEY)?.ciphertext !== record.ciphertext) {
      throw new StorageError(STORAGE_UNAVAILABLE);
    }
    this.saved = record;
    this.changed();
  }

  async unlockSaved(passphrase: string, glue: KeyGlue) {
    if (!this.saved) throw new Error("No saved key.");
    this.hold(await decryptKey(this.saved, passphrase, glue));
  }

  deleteSaved() {
    removeStored("local", SAVED_KEY);
    this.saved = null;
    this.changed();
  }
}

export const keyStore = new KeyStore();
