import * as React from "react";

import { Boot, type BootSnapshot } from "../boot/boot";
import { CONFIG } from "../config";
import { gate } from "./load-gate";

// One node per page: a second boot would run a second node.
const boot = new Boot(CONFIG);

// The key a role change carried was taken into memory at the load
// (load-gate.ts), so that it can't expire while the start screen waits.
boot.useCarried(gate.carried);

/** The page navigated to start the node again (a role change, the store's
 * reset, a retry). Otherwise the start screen asks. */
export const autostart = gate.autostart;

/** The boot's actions, for the producer's unlock at its restart. */
export const bootActions = {
  unlockProducer: boot.unlockProducer,
  startWithoutRole: boot.startWithoutRole,
};

/** Starts the node, once: the page's only start. */
export function startNode() {
  boot.start();
}

export function useBoot(): BootSnapshot {
  return React.useSyncExternalStore(boot.subscribe, boot.getSnapshot);
}
