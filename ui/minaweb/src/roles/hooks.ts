/** The role screens' hooks: the tab's key, this node's role calls, and
 * the key's account, polled. */

import * as React from "react";

import { usePoll, type Polled } from "@minanode/node-client/react";

import { useBoot } from "../app/boot-context";
import { keyStore, type KeyGlue } from "./keys";
import { RoleNode, type Account, type RoleNodeRpc } from "./node";

export function useKeys() {
  return React.useSyncExternalStore(keyStore.subscribe, keyStore.getSnapshot);
}

export function useRoleNode(): RoleNode | null {
  const { rpc } = useBoot();
  return React.useMemo(
    () => (rpc ? new RoleNode(rpc as unknown as RoleNodeRpc) : null),
    [rpc],
  );
}

/** The WASM's key functions, when this build has them. */
export function useKeyGlue(): KeyGlue | null {
  const { glue } = useBoot();
  if (
    !glue ||
    typeof glue.generate_account_key !== "function" ||
    typeof glue.account_public_key !== "function" ||
    typeof glue.account_public_key_valid !== "function"
  ) {
    return null;
  }
  return glue as KeyGlue;
}

export const ACCOUNT_POLL_MS = 10_000;

/** A key's account at the best tip: null if it doesn't exist (or the
 * node has no best tip yet). */
export function useAccount(publicKey: string | null): Polled<Account | null> {
  const node = useRoleNode();
  return usePoll(
    node?.available && publicKey ? `account:${publicKey}` : null,
    () => node!.account(publicKey!),
    ACCOUNT_POLL_MS,
  );
}
