/**
 * The boot's six steps, in order: what the start screen shows waiting
 * before Start, and the boot then runs (boot.ts). Each waiting step shows a
 * figure from the build instead of its progress.
 */

import type { BuildConfig } from "./build-config";
import { formatBytes } from "./failures";
import { relayShortNames } from "../lib/peers";

export type StepId =
  "check" | "download" | "start" | "connect" | "ledgers" | "blocks";

export type StepState = "done" | "current" | "stopped" | "skipped" | "pending";

export interface Step {
  id: StepId;
  label: string;
  state: StepState;
  detail: string | null;
  startedAt: number | null;
  endedAt: number | null;
}

const LABELS: Record<StepId, string> = {
  check: "Check the browser",
  download: "Download the node",
  start: "Compile and start the node",
  connect: "Connect to a peer",
  ledgers: "Sync the ledgers",
  blocks: "Sync the blocks",
};

/** The steps before the boot starts: every one waiting. */
export function pendingSteps(): Step[] {
  return (Object.keys(LABELS) as StepId[]).map(id => ({
    id,
    label: LABELS[id],
    state: "pending",
    detail: null,
    startedAt: null,
    endedAt: null,
  }));
}

/** Each step's figure while it waits, from the build's config. */
export function stepFigures(config: BuildConfig): Record<StepId, string> {
  const { bytes, compressedBytes } = config.wasm;
  const relays = relayShortNames(config.relays);
  return {
    check: "Cross-origin isolation, WebAssembly threads",
    download:
      bytes === null
        ? "WebAssembly, SHA-256 checked"
        : compressedBytes === null
          ? formatBytes(bytes)
          : `${formatBytes(bytes)}, about ${formatBytes(compressedBytes)} compressed`,
    start: "In this tab",
    connect: relays.length ? relays.join(" · ") : "From the seed list",
    ledgers: relays.length ? "From the relays" : "From peers",
    blocks: "To the tip",
  };
}
