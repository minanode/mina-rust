/**
 * `GET /stats/verification`: the checks a node ran on each block it
 * received (crates/node/src/verification), newest first. A web node answers
 * the same from `stats().verification()`. Times are ms since the epoch.
 */

import { arr, bool, num, obj, str } from "./json";

/** Receipts a page asks for by default. */
export const VERIFICATION_PAGE = 20;
/** The most the node gives in one answer. */
export const VERIFICATION_MAX_PAGE = 128;

export type CheckStatus =
  | "passed"
  | "failed"
  | "pending"
  | "skipped"
  | "notRun";

export interface Check {
  status: CheckStatus;
  checkedAtMs: number | null;
  elapsedMs: number | null;
}

export interface Receipt {
  stateHash: string;
  height: number | null;
  /** How the block came: gossip, catchup, bootstrap, bestTip or local. */
  arrival: string | null;
  receivedAtMs: number | null;
  /** The node applied the block to its frontier. */
  applied: boolean;
  /** Its SNARK proof, checked against the verifier key. */
  proof: Check;
  /** The delta chain proof that links it to its parent. */
  deltaChain: Check;
  /** The body matches the hash the header commits to. */
  body: Check;
}

export interface Verification {
  verifiedSinceStart: number | null;
  failuresSinceStart: number | null;
  appliedSinceStart: number | null;
  medianProofTimeMs: number | null;
  /** The newest block whose proof passed. */
  lastVerified: Receipt | null;
  receipts: Receipt[];
}

const STATUSES = new Set<string>([
  "passed",
  "failed",
  "pending",
  "skipped",
  "notRun",
]);

function check(value: unknown): Check {
  const c = obj(value) ?? {};
  const status = str(c.status);
  return {
    status: status && STATUSES.has(status) ? (status as CheckStatus) : "notRun",
    checkedAtMs: num(c.checkedAt),
    elapsedMs: num(c.elapsedMs),
  };
}

export function parseReceipt(value: unknown): Receipt | null {
  const r = obj(value);
  const block = obj(r?.block);
  const stateHash = str(block?.stateHash);
  if (!r || !stateHash) return null;
  return {
    stateHash,
    height: num(block?.height),
    arrival: str(r.arrival),
    receivedAtMs: num(r.receivedAt),
    applied: bool(r.applied) ?? false,
    proof: check(r.blockProof),
    deltaChain: check(r.deltaChain),
    body: check(r.bodyReference),
  };
}

export function parseVerification(raw: unknown): Verification {
  const v = obj(raw) ?? {};
  return {
    verifiedSinceStart: num(v.verifiedSinceStart),
    failuresSinceStart: num(v.failuresSinceStart),
    appliedSinceStart: num(v.appliedSinceStart),
    medianProofTimeMs: num(v.medianProofTimeMs),
    lastVerified: parseReceipt(v.lastVerified),
    receipts: arr(v.receipts)
      .map(parseReceipt)
      .filter((r): r is Receipt => r !== null),
  };
}

/** A receipt's checks in a word: failed if any failed, pending while any
 * runs, passed once the proof passed. */
export function receiptOutcome(
  receipt: Receipt,
): "passed" | "failed" | "pending" | "unchecked" {
  const checks = [receipt.proof, receipt.deltaChain, receipt.body];
  if (checks.some(c => c.status === "failed")) return "failed";
  if (checks.some(c => c.status === "pending")) return "pending";
  if (receipt.proof.status === "passed") return "passed";
  return "unchecked";
}
