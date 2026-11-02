/**
 * The page starts the node only when the visitor asks, on the start screen,
 * except after a navigation the page makes itself for the node it already
 * runs: a role change (roles/restart.ts), the frontier store's reset, or a
 * retry from a failure. No clock decides it. Each of those writes a fresh
 * random one-time nonce to this tab's sessionStorage, then replaces the page
 * with the same address carrying the nonce in the query
 * (`?autostart=<nonce>`; a change of the hash alone wouldn't load the page
 * again).
 *
 * At load, before anything else, the page reads both, deletes the stored
 * nonce and removes the parameter from the address (history.replaceState,
 * keeping the path, the rest of the query and the hash). It starts the node
 * by itself only if both were there and match.
 *
 * - A manual refresh after a page-made navigation has no parameter (it was
 *   removed): the screen.
 * - A page-made navigation that was stopped leaves the old address, with no
 *   parameter; the stored nonce is deleted at the next load: the screen.
 * - A duplicated tab copies the cleaned address: the screen.
 * - An address with a parameter but no stored nonce (pasted, shared,
 *   bookmarked, another tab): the screen.
 *
 * What's left: a tab duplicated in the moment between the new page's
 * address showing and this module running, and a refresh of a new page that
 * failed before this module ran (an error page at the nonce address), which
 * completes the page's own navigation.
 *
 * A carried key (roles/handoff.ts) is taken into memory at load either way
 * (IndexedDB and WebCrypto only, nothing downloaded), so it can't expire
 * while the screen waits. A new visit shows the screen, with a chain saved
 * in this browser or not.
 */

import { parseHandoff, TAB_HANDOFF, takeHandoff } from "../roles/handoff";
import { keyStore, writeVerified, type HeldKey } from "../roles/keys";

/** sessionStorage: the nonce the next page-made navigation carries. */
export const AUTOSTART_KEY = "minaweb.autostart";
/** The query parameter that carries it. */
export const AUTOSTART_PARAM = "autostart";

/** 128 random bits, as 32 hex digits. */
const NONCE = /^[0-9a-f]{32}$/;

export function newNonce(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
}

/** `href` with the nonce in its query, its path and hash kept. */
export function autostartUrl(href: string, nonce: string): string {
  const url = new URL(href);
  url.searchParams.set(AUTOSTART_PARAM, nonce);
  return url.href;
}

function session(): Storage | null {
  try {
    return sessionStorage;
  } catch {
    return null;
  }
}

/** Takes the stored nonce back, for a navigation that won't happen. */
export function clearAutostart() {
  try {
    session()?.removeItem(AUTOSTART_KEY);
  } catch {
    // Nothing stored.
  }
}

/** The page's own navigation, with this nonce. */
export type Go = (nonce: string) => void;

/** Replaces the page with this address carrying the nonce. */
export const replaceWithNonce: Go = nonce =>
  location.replace(autostartUrl(location.href, nonce));

/** Keeps a nonce for the navigation about to happen, or throws
 * StorageError, for one that must not wait on the screen (a role change:
 * a producer would stall). Returns the nonce. */
export function markAutostartVerified(nonce = newNonce()): string {
  writeVerified("session", AUTOSTART_KEY, nonce);
  return nonce;
}

/** Loads the page again as its own: the next load starts the node. Best
 * effort: without storage, the next load shows the screen. */
export function reloadAndStart(go: Go = replaceWithNonce) {
  const nonce = newNonce();
  try {
    session()?.setItem(AUTOSTART_KEY, nonce);
  } catch {
    // The screen shows; Start goes on from there.
  }
  go(nonce);
}

export interface Autostart {
  /** Start the node without the screen. */
  autostart: boolean;
  /** The address carried a nonce. */
  param: boolean;
  /** This tab had one stored. */
  stored: boolean;
}

/** What `takeAutostart` changes the address with. */
export type ReplaceUrl = (url: string) => void;

const replaceUrl: ReplaceUrl = url =>
  history.replaceState(history.state, "", url);

/**
 * Reads this load's nonces: the address's and this tab's. Deletes the
 * stored one and removes the parameter from the address, whatever they
 * hold. Starts by itself only if both are there and match.
 */
export function takeAutostart(
  href: string = location.href,
  replace: ReplaceUrl = replaceUrl,
): Autostart {
  let given: string | null = null;
  try {
    const url = new URL(href);
    given = url.searchParams.get(AUTOSTART_PARAM);
    if (url.searchParams.has(AUTOSTART_PARAM)) {
      url.searchParams.delete(AUTOSTART_PARAM);
      replace(url.pathname + url.search + url.hash);
    }
  } catch {
    given = null;
  }
  let stored: string | null = null;
  const s = session();
  try {
    stored = s?.getItem(AUTOSTART_KEY) ?? null;
    s?.removeItem(AUTOSTART_KEY);
  } catch {
    // Without storage there is no stored nonce: the screen.
    stored = null;
  }
  return {
    autostart: given !== null && NONCE.test(given) && given === stored,
    param: given !== null,
    stored: stored !== null,
  };
}

export interface LoadGate extends Autostart {
  /** The carried key, being taken into memory; null without one. */
  carried: Promise<HeldKey | null> | null;
}

/** At the page's load, before anything else reads the address or this
 * tab's storage: whether the node starts by itself, and the carried key
 * taken into memory. */
export function loadGate(
  href: string = location.href,
  replace: ReplaceUrl = replaceUrl,
  take: () => Promise<HeldKey | null> = () => takeHandoff(),
  hold: (key: HeldKey) => void = key => keyStore.hold(key),
): LoadGate {
  const gate = takeAutostart(href, replace);
  let record = null;
  try {
    record = parseHandoff(session()?.getItem(TAB_HANDOFF) ?? null);
  } catch {
    record = null;
  }
  const carried =
    record === null
      ? null
      : take()
          .catch(() => null)
          .then(key => {
            if (key) hold(key);
            return key;
          });
  return { ...gate, carried };
}
