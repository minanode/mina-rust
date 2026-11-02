import * as React from "react";

import { Boot, type BootSnapshot } from "../boot/boot";
import { CONFIG } from "../config";

// One node per page: a second boot would run a second node.
const boot = new Boot(CONFIG);

/** The boot's actions, for the producer's unlock at its restart. */
export const bootActions = {
  unlockProducer: boot.unlockProducer,
  startWithoutRole: boot.startWithoutRole,
};

export function useBoot(): BootSnapshot {
  React.useEffect(() => boot.start(), []);
  return React.useSyncExternalStore(boot.subscribe, boot.getSnapshot);
}
