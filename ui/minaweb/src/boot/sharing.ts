/**
 * Sharing with other browsers (MINAWEB-OPT-IN-SHARING): off unless the
 * visitor turns it on. The choice is this browser's (localStorage), and the
 * node reads it only when it starts (`run`'s `sharing` option), so turning
 * it on or off starts the node again through the page's own restart
 * (roles/restart.ts): the tab's key and role cross it.
 */

import { StorageError, writeVerified } from "../roles/keys";

export const SHARING_KEY = "minaweb.sharing";

/** The most browsers a sharing node connects to, besides its relays: the
 * node's own cap (`P2pSharingPeerLimits::default().max_peers` in
 * crates/p2p/src/sharing.rs). */
export const SHARING_MAX_PEERS = 4;

function local(): Storage | null {
  try {
    return localStorage;
  } catch {
    return null;
  }
}

/** Whether the visitor turned sharing on in this browser. */
export function readSharing(): boolean {
  try {
    return local()?.getItem(SHARING_KEY) === "on";
  } catch {
    return false;
  }
}

/** Keeps the choice for the next start. Throws StorageError unless it is
 * kept: a restart must not start the node with the old choice. */
export function writeSharing(on: boolean) {
  if (on) {
    writeVerified("local", SHARING_KEY, "on");
    return;
  }
  try {
    local()?.removeItem(SHARING_KEY);
  } catch {
    // Checked below.
  }
  if (readSharing()) {
    throw new StorageError(
      "This browser's storage didn't take the change, so sharing is still on.",
    );
  }
}
