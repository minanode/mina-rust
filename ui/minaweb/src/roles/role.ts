/**
 * The tab's role: what its node does besides following the chain. A web
 * node runs the SNARK worker or the block producer, never both (P5.3), and
 * the role is fixed when the node starts (`run`'s arguments), so choosing
 * or changing one starts the node again: the page reloads, and its store
 * brings the chain back. The choice is this tab's (sessionStorage), and
 * holds no private key: the worker needs only a public key, and the
 * producer's key crosses the restart encrypted (keys.ts, HANDOFF_KEY).
 *
 * Devnet only: on any other network the page has no roles, whatever the
 * storage says.
 */

import type { BuildConfig } from "../boot/build-config";
import { looksLikePublicKey, writeVerified } from "./keys";

export type Role =
  | { kind: "viewer" }
  | { kind: "worker"; publicKey: string; fee: string }
  | { kind: "producer"; publicKey: string };

export type RoleKind = Role["kind"];

export const ROLE_KEY = "minaweb.role";

export const VIEWER: Role = { kind: "viewer" };

/** Roles exist on devnet only. */
export function rolesAllowed(network: BuildConfig["network"]): boolean {
  return network === "devnet";
}

/** A fee as `run` takes it: MINA, up to nine decimals, positive. */
const FEE = /^(?:0|[1-9]\d{0,9})(?:\.\d{1,9})?$/;

export function parseRole(text: string | null, network: string): Role {
  if (!text || network !== "devnet") return VIEWER;
  try {
    const r = JSON.parse(text) as Record<string, unknown>;
    if (
      r.kind === "worker" &&
      typeof r.publicKey === "string" &&
      looksLikePublicKey(r.publicKey) &&
      typeof r.fee === "string" &&
      FEE.test(r.fee) &&
      Number(r.fee) > 0
    ) {
      return { kind: "worker", publicKey: r.publicKey, fee: r.fee };
    }
    if (
      r.kind === "producer" &&
      typeof r.publicKey === "string" &&
      looksLikePublicKey(r.publicKey)
    ) {
      return { kind: "producer", publicKey: r.publicKey };
    }
  } catch {
    // Not a role of ours.
  }
  return VIEWER;
}

function session(): Storage | null {
  try {
    return sessionStorage;
  } catch {
    return null;
  }
}

export function readRole(network: string): Role {
  try {
    return parseRole(session()?.getItem(ROLE_KEY) ?? null, network);
  } catch {
    return VIEWER;
  }
}

/** Keeps the role for the restart; throws StorageError unless it is kept
 * (a reload would otherwise start without it). */
export function writeRole(role: Role) {
  if (role.kind === "viewer") {
    try {
      session()?.removeItem(ROLE_KEY);
    } catch {
      // No storage: no role either, which is the viewer.
    }
    return;
  }
  writeVerified("session", ROLE_KEY, JSON.stringify(role));
}

export interface RunRoleArgs {
  blockProducer: string | null;
  snarker: string | null;
  snarkerFee: string | null;
}

/** `run`'s role arguments. The producer's private key is passed only for
 * a producer role whose public key it matches. */
export function runRoleArgs(
  role: Role,
  producer: { publicKey: string; privateKey: string } | null,
): RunRoleArgs {
  switch (role.kind) {
    case "worker":
      return {
        blockProducer: null,
        snarker: role.publicKey,
        snarkerFee: role.fee,
      };
    case "producer":
      if (!producer || producer.publicKey !== role.publicKey) {
        throw new Error("The producer's key isn't the one this tab chose.");
      }
      return {
        blockProducer: producer.privateKey,
        snarker: null,
        snarkerFee: null,
      };
    case "viewer":
      return { blockProducer: null, snarker: null, snarkerFee: null };
  }
}

/**
 * `run`'s options object, from the build alone: the role never changes
 * it. In particular it never turns on `peerDiscovery` (PR #173): the node
 * dials the build's seed relays only and takes no incoming connections,
 * whatever it does.
 */
export function runOptions(
  frontierStore: boolean,
  iceServers: unknown[] | null,
  frontierStoreReset = false,
): Record<string, unknown> {
  return {
    frontierStore,
    ...(frontierStoreReset ? { frontierStoreReset: true } : {}),
    ...(iceServers ? { iceServers } : {}),
  };
}

export const ROLE_LABELS: Record<RoleKind, string> = {
  viewer: "Following the chain",
  worker: "SNARK worker",
  producer: "Block producer",
};
