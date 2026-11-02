/**
 * Every role change starts the node again by reloading the page, and a
 * reload empties the tab's memory. `restartWithKey` is the one way the
 * page restarts. The key the tab holds crosses the reload by itself
 * (handoff.ts) and is held again after it, with no passphrase. Where the
 * browser can't do that, it throws HandoffError without reloading, and the
 * page falls back to a choice: carry the key encrypted with a passphrase
 * (in this tab's sessionStorage), or, for a key saved here or one the
 * visitor lets go, restart without it. A reload whose key or role didn't
 * reach storage is refused.
 */

import {
  clearAutostart,
  markAutostartVerified,
  replaceWithNonce,
  type Go,
} from "../boot/autostart";
import { carryHeld, discardHandoff } from "./handoff";
import {
  encryptKey,
  HANDOFF_KEY,
  readStored,
  writeStored,
  type EncryptedKey,
  type HeldKey,
} from "./keys";
import { writeRole, type Role } from "./role";

/** Only where the key can't cross the reload by itself. */
export type KeyChoice =
  /** Carry it across, encrypted with this passphrase. */
  | { kind: "carry"; passphrase: string }
  /** Restart without it: it is saved here, or the visitor lets it go. */
  | { kind: "drop" };

/** The tab holds a key that a reload would lose: it isn't saved here. */
export function keyAtRisk(
  key: HeldKey | null,
  saved: EncryptedKey | null,
): key is HeldKey {
  return key !== null && saved?.publicKey !== key.publicKey;
}

/**
 * Keeps what the restart needs, then reloads. With no choice, the held key
 * crosses by itself, or HandoffError is thrown. Throws, without reloading,
 * if storage refuses the key's copy, the start nonce or the role.
 */
export async function restartWithKey(
  role: Role,
  key: HeldKey | null,
  choice: KeyChoice | null,
  go: Go = replaceWithNonce,
  carry: (key: HeldKey) => Promise<void> = carryHeld,
  discard: () => Promise<void> = discardHandoff,
) {
  // The producer needs its key at the start.
  const producer = role.kind === "producer";
  if (producer && key?.publicKey !== role.publicKey) {
    throw new Error("The producer's key isn't the one this tab holds.");
  }
  let carried = false;
  if (key) {
    if (choice === null) {
      await carry(key);
      carried = true;
    } else if (choice.kind === "drop") {
      if (producer)
        throw new Error("The producer needs its key across the restart.");
    } else {
      const record = await encryptKey(key, choice.passphrase);
      writeStored("session", HANDOFF_KEY, record);
      if (
        readStored("session", HANDOFF_KEY)?.ciphertext !== record.ciphertext
      ) {
        throw new Error("The key's encrypted copy didn't reach storage.");
      }
    }
  }
  // The node starts again by itself after the navigation, with no start
  // screen (a producer would stall on it): a restart whose nonce or role
  // doesn't reach storage doesn't happen.
  let nonce: string;
  try {
    nonce = markAutostartVerified();
    writeRole(role);
  } catch (error) {
    clearAutostart();
    if (carried) await discard();
    throw error;
  }
  go(nonce);
}
