/**
 * The node's WASM: fetched with its progress, then checked against the
 * SHA-256 the page was built with. Bytes that don't match are never
 * compiled, so a WASM from a compromised or stale host can't run.
 */

import type { BootFailure } from "./failures";

export interface DownloadProgress {
  received: number;
  /** The WASM's size, from the build or the answer's headers. */
  total: number | null;
}

export class BootError extends Error {
  readonly failure: BootFailure;
  constructor(failure: BootFailure) {
    super(failure.title);
    this.failure = failure;
  }
}

export async function sha256Hex(
  bytes: ArrayBuffer,
  subtle: SubtleCrypto = globalThis.crypto.subtle,
): Promise<string> {
  const digest = new Uint8Array(await subtle.digest("SHA-256", bytes));
  return Array.from(digest, b => b.toString(16).padStart(2, "0")).join("");
}

/** The answer's size when the body is sent as it is; a compressed body's
 * Content-Length is not the size of what the reader gets. */
function sentSize(response: Response): number | null {
  const encoding = response.headers.get("Content-Encoding");
  if (encoding && encoding !== "identity") return null;
  const length = Number(response.headers.get("Content-Length"));
  return Number.isFinite(length) && length > 0 ? length : null;
}

/** The compressed size the server sent `url` at, from the browser's
 * resource timing; null when the browser doesn't say, or the body wasn't
 * compressed. */
export function compressedSize(
  url: string,
  bytes: number,
  timing:
    | Pick<Performance, "getEntriesByName">
    | undefined = globalThis.performance,
): number | null {
  try {
    const href = new URL(url, globalThis.location?.href).href;
    const entries = timing?.getEntriesByName(href, "resource") ?? [];
    const last = entries.at(-1) as PerformanceResourceTiming | undefined;
    const size = last?.encodedBodySize ?? 0;
    return size > 0 && size < bytes ? size : null;
  } catch {
    return null;
  }
}

export async function downloadWasm(
  wasm: { url: string; sha256: string; bytes: number | null },
  onProgress: (progress: DownloadProgress) => void,
  fetchFn: (input: string) => Promise<Response> = input => fetch(input),
): Promise<ArrayBuffer> {
  let response: Response;
  try {
    response = await fetchFn(wasm.url);
  } catch (error) {
    throw new BootError({
      kind: "download",
      title: "The node's download didn't start",
      detail: `No answer for ${wasm.url}: ${error instanceof Error ? error.message : String(error)}.`,
      hint: "Check the connection, then reload the page.",
    });
  }
  if (!response.ok) {
    throw new BootError({
      kind: "files",
      title: "The node's WASM isn't on the server",
      detail: `${wasm.url} answered HTTP ${response.status}.`,
      hint: "The page and its WASM come from different builds, or the WASM wasn't uploaded.",
    });
  }
  const total = wasm.bytes ?? sentSize(response);
  let received = 0;
  onProgress({ received, total });
  const chunks: Uint8Array[] = [];
  try {
    if (!response.body) {
      const all = new Uint8Array(await response.arrayBuffer());
      chunks.push(all);
      received = all.length;
    } else {
      const reader = response.body.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        received += value.length;
        onProgress({ received, total });
      }
    }
  } catch (error) {
    throw new BootError({
      kind: "download",
      title: "The node's download broke off",
      detail: `After ${received} bytes of ${wasm.url}: ${error instanceof Error ? error.message : String(error)}.`,
      hint: "Reload the page to try again.",
    });
  }
  onProgress({ received, total });
  const bytes = new Uint8Array(received);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.length;
  }
  const got = await sha256Hex(bytes.buffer);
  if (got !== wasm.sha256) {
    throw new BootError({
      kind: "integrity",
      title: "The node's WASM failed its integrity check",
      detail: `Its SHA-256 is ${got}. This page was built for ${wasm.sha256}. The page doesn't run it.`,
      hint: "The server has another build's WASM, or the file was changed. Reload once; if it stays, tell whoever runs this site.",
    });
  }
  return bytes.buffer;
}
