/**
 * What each role costs the visitor's device, shown on its setup screen
 * before Start: the prover circuits it downloads, and the tab's peak memory
 * (node and prover together), measured in headless Chrome on devnet. A
 * device that reports less memory than a role is comfortable with gets a
 * warning, not a refusal: `navigator.deviceMemory` is rounded and capped
 * at 8 GB, and absent outside Chromium.
 */

import type { RoleKind } from "./role";

export interface RoleCost {
  /** Prover circuit files the role downloads, in bytes (once per start;
   * the verifier files every node loads aren't counted). */
  circuitBytes: number;
  /** The tab's peak memory with the role at work, in bytes. */
  peakBytes: number;
  /** What the peak was measured over. */
  peakBasis: string;
  /** Seconds from the node's start to the prover being ready. */
  setupSeconds: number;
}

const GIB = 1024 ** 3;

/**
 * Two headless runs on devnet, build ca9dcd6f6 (MINAWEB-P2-WRITE-roles.md):
 * circuit bytes from the static server's log, peaks as the sum of PSS over
 * Chrome's processes, readiness from the node's start to "isolated block
 * prover ready". The worker's peak is over 20 bundles proved. A key without
 * stake wins no slot, so the producer's peak is its measured peak with the
 * prover ready (2.29 GiB) plus the growth of a block proof as I34c measured
 * it (876 to 2,610 MiB of prover memory, +1.69 GiB).
 */
export const ROLE_COSTS: Record<Exclude<RoleKind, "viewer">, RoleCost> = {
  worker: {
    circuitBytes: 49_495_786,
    peakBytes: 3_328_143_360,
    peakBasis: "while proving SNARK work",
    setupSeconds: 31,
  },
  producer: {
    circuitBytes: 45_441_376,
    peakBytes: 2_454_714_368 + 1_733 * 1024 ** 2,
    peakBasis: "while proving a block (estimated)",
    setupSeconds: 41,
  },
};

/** A role change starts the node again; for about 30 s the tab still
 * holds the old node's memory too: 4.07 GiB measured stopping the worker. */
export const SWITCH_PEAK_BYTES = 4_365_904_896;

/** The device memory below which a role gets a warning, in GB as
 * `navigator.deviceMemory` reports it. */
export const COMFORTABLE_GB = 8;

export function deviceMemoryGb(): number | null {
  const value = (navigator as { deviceMemory?: unknown }).deviceMemory;
  return typeof value === "number" && value > 0 ? value : null;
}

/** A warning for a device that says it has little memory; null when it
 * has enough, or doesn't say. */
export function lowMemoryWarning(
  kind: Exclude<RoleKind, "viewer">,
  reportedGb: number | null,
): string | null {
  if (reportedGb === null || reportedGb >= COMFORTABLE_GB) return null;
  const peak = (
    Math.max(ROLE_COSTS[kind].peakBytes, SWITCH_PEAK_BYTES) / GIB
  ).toFixed(1);
  return `This device reports about ${reportedGb} GB of memory. The ${kind === "worker" ? "SNARK worker" : "block producer"} peaks at about ${peak} GiB in this tab, with its start, so the browser may stop the tab, or the device may slow down. You can still start it.`;
}
