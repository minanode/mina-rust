/** The web node's heap counters (crates/node/web/src/memory.rs): what it
 * holds now, its peak, and the linear memory the browser reserved, which
 * only grows. */

import { num, obj } from "@minanode/node-client";

export interface MemoryStats {
  liveBytes: number;
  peakLiveBytes: number | null;
  wasmBytes: number | null;
}

export function parseMemoryStats(raw: unknown): MemoryStats | null {
  const m = obj(raw);
  const live = num(m?.live_bytes);
  if (!m || live === null) return null;
  return {
    liveBytes: live,
    peakLiveBytes: num(m.peak_live_bytes),
    wasmBytes: num(m.wasm_bytes),
  };
}
