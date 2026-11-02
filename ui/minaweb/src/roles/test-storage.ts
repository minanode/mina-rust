/** Tests only: a Map-backed Storage, and storage that refuses access or
 * writes, installed as the page's localStorage and sessionStorage. */

export class MemoryStorage implements Storage {
  private map = new Map<string, string>();
  refuseWrites = false;
  get length() {
    return this.map.size;
  }
  clear() {
    this.map.clear();
  }
  getItem(key: string) {
    return this.map.get(key) ?? null;
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  setItem(key: string, value: string) {
    if (this.refuseWrites) {
      throw new DOMException(
        "The quota has been exceeded.",
        "QuotaExceededError",
      );
    }
    this.map.set(key, value);
  }
}

type Kind = "localStorage" | "sessionStorage";

/** Installs `storage` (or, for "denied", a getter that throws as in a
 * browser with storage turned off). */
export function installStorage(kind: Kind, storage: Storage | "denied") {
  Object.defineProperty(globalThis, kind, {
    configurable: true,
    get() {
      if (storage === "denied") {
        throw new DOMException("The operation is insecure.", "SecurityError");
      }
      return storage;
    },
  });
}
