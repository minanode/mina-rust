/**
 * The frontier store as the pages show it: the run's choice (boot.ts) and
 * what the node's `/status` says of it. A second tab of the same browser
 * can't open the store, which the first tab holds (the Web Lock in
 * crates/node/web/src/frontier_store.rs), so it runs a node of its own
 * without the saved chain.
 */

import type { FrontierStore } from "@minanode/node-client";

export type StoreState =
  /** Off for this run, by the build or the visitor's choice. */
  | { kind: "off" }
  /** Another tab of this browser holds the store. */
  | { kind: "other-tab" }
  /** The node turned the store off for this run, for `reason`. */
  | { kind: "unavailable"; reason: string }
  /** On, and not open yet, or the node hasn't said. */
  | { kind: "opening" }
  /** Open in this tab. */
  | { kind: "on" };

export function storeState(
  on: boolean,
  store: FrontierStore | null | undefined,
): StoreState {
  if (!on) return { kind: "off" };
  if (!store) return { kind: "opening" };
  if (store.locked) return { kind: "other-tab" };
  if (store.unavailable !== null) {
    return { kind: "unavailable", reason: store.unavailable };
  }
  return store.open ? { kind: "on" } : { kind: "opening" };
}

/** The store's status, as one value. */
export function storeLabel(state: StoreState): string {
  switch (state.kind) {
    case "off":
      return "Off";
    case "other-tab":
    case "unavailable":
      return "Not available in this tab";
    case "opening":
      return "Opening";
    case "on":
      return "On";
  }
}

/** The docs section on the saved chain. */
export const SAVED_CHAIN_DOCS_URL =
  "https://minanode.org/docs/web-node-connections/#saved-chain";
