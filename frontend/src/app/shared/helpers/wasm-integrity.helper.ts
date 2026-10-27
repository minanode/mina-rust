/**
 * The web node's WASM, checked against the digest this page was built with
 * (W1 fix round). A build that loads the WASM from another host (WASM_URL in
 * tools/regression/webnode/build.sh, for a static host that caps file sizes)
 * pins its URL and SHA-256 in the page, served from the site. The page
 * fetches the bytes, checks them with WebCrypto and only then hands them to
 * the glue to compile: bytes that don't match are never run, so a node that
 * may hold a block producer's key never starts on them. A build without a pin
 * (the WASM next to the page, as before) loads it as before.
 */
export interface PinnedWasm {
  url: string;
  sha256: string;
}

export class WasmIntegrityError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'WasmIntegrityError';
  }
}

/**
 * The pin of a build: null without one; an error for a URL without a valid
 * SHA-256, or a SHA-256 without a URL, so that a half-configured build
 * refuses to load the WASM rather than load it unchecked.
 */
export function parsePinnedWasm(
  url: string | undefined,
  sha256: string | undefined,
): PinnedWasm | null {
  const u = (url ?? '').trim();
  const d = (sha256 ?? '').trim().toLowerCase();
  if (!u && !d) return null;
  if (!u || !/^[0-9a-f]{64}$/.test(d)) {
    throw new WasmIntegrityError(
      `this page pins the web node's WASM badly (URL "${u}", SHA-256 "${d}"): not starting the node`,
    );
  }
  return { url: u, sha256: d };
}

export async function sha256Hex(
  bytes: ArrayBuffer,
  subtle: SubtleCrypto = globalThis.crypto.subtle,
): Promise<string> {
  const digest = new Uint8Array(await subtle.digest('SHA-256', bytes));
  return Array.from(digest, b => b.toString(16).padStart(2, '0')).join('');
}

/**
 * The pinned WASM's bytes, once their SHA-256 is the pinned one. Throws a
 * WasmIntegrityError, saying both digests, for other bytes.
 */
export async function fetchPinnedWasm(
  pin: PinnedWasm,
  fetchFn: (input: string) => Promise<Response> = input => fetch(input),
  subtle?: SubtleCrypto,
): Promise<ArrayBuffer> {
  const response = await fetchFn(pin.url);
  if (!response.ok) {
    throw new WasmIntegrityError(
      `the web node's WASM ${pin.url}: HTTP ${response.status}`,
    );
  }
  const bytes = await response.arrayBuffer();
  const got = await sha256Hex(bytes, subtle);
  if (got !== pin.sha256) {
    throw new WasmIntegrityError(
      `the web node's WASM from ${pin.url} is not the one this page was built with ` +
        `(SHA-256 ${got}, expected ${pin.sha256}): not starting the node`,
    );
  }
  return bytes;
}
