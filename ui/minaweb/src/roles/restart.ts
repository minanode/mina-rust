/**
 * Every role change starts the node again by reloading the page, and a
 * reload empties the tab's memory. So a key held only there would be lost.
 * `restartWithKey` is the one way the page restarts: a key not saved in
 * this browser either crosses the reload encrypted with a passphrase (in
 * this tab's sessionStorage, as the producer's does), or the visitor says
 * plainly to restart without it. Anything else is refused, and so is a
 * reload whose encrypted copy or role didn't reach storage.
 */

import {
  encryptKey,
  HANDOFF_KEY,
  readStored,
  writeStored,
  type EncryptedKey,
  type HeldKey,
} from "./keys";
import { writeRole, type Role } from "./role";

export type KeyChoice =
  /** Carry it across, encrypted with this passphrase. */
  | { kind: "carry"; passphrase: string }
  /** Restart without it: the visitor has a backup, or doesn't need it. */
  | { kind: "drop" };

/** The tab holds a key that a reload would lose: it isn't saved here. */
export function keyAtRisk(
  key: HeldKey | null,
  saved: EncryptedKey | null,
): key is HeldKey {
  return key !== null && saved?.publicKey !== key.publicKey;
}

export const KEY_CHOICE_NEEDED =
  "This tab's key isn't saved: carry it across the restart with a passphrase, or confirm restarting without it.";

/**
 * Keeps what the restart needs, then reloads. Throws, without reloading,
 * if the key would be lost unasked, or if storage refuses its copy or the
 * role.
 */
export async function restartWithKey(
  role: Role,
  key: HeldKey | null,
  saved: EncryptedKey | null,
  choice: KeyChoice | null,
  reload: () => void = () => location.reload(),
) {
  // The producer needs its key at the start: it is always carried.
  const producer = role.kind === "producer";
  if (producer && key?.publicKey !== role.publicKey) {
    throw new Error("The producer's key isn't the one this tab holds.");
  }
  if (key && (producer || keyAtRisk(key, saved))) {
    if (!choice) throw new Error(KEY_CHOICE_NEEDED);
    if (choice.kind === "drop") {
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
  writeRole(role);
  reload();
}
