/**
 * The sync phase in an operator's words, with the checks the Angular app's
 * freshness helper makes (R26c, F-dash3): the best tip block's own time, the
 * node's last sync event and the dashboard's last answer are three different
 * clocks, and a node that says Synced on a tip that can't be the chain's is
 * shown as not synced.
 */

import { formatAge, formatHeight, plural } from "./format";

export type Tone = "ok" | "info" | "warn" | "error" | "neutral";

/** The dashboard asks for /status every 5 s; four missed answers is stale. */
export const STATUS_POLL_MS = 5_000;
export const STATUS_STALE_MS = 20_000;
/** Mesa has a block every three minutes or so; past this, say so. */
export const TIP_QUIET_MS = 15 * 60_000;
/** A Synced tip older than this, or this far below a peer's, isn't the
 * chain's tip. */
export const SYNCED_TIP_MAX_AGE_MS = 60 * 60_000;
export const SYNCED_MAX_BEHIND_PEERS = 10;
/** Consensus k on Mina mainnet and devnet: a gap wider than this is not
 * caught up block by block; the node fetches ledgers first. */
export const K = 290;

export interface SyncInput {
  /** The phase /status reports, or null when the node has never answered. */
  phase: string | null;
  bestTipHeight: number | null;
  /** The best tip block's timestamp, ms since the epoch. */
  bestTipTimestampMs: number | null;
  targetHeight: number | null;
  /** The highest best tip a connected peer reports. */
  highestPeerTip: number | null;
  /** When the dashboard last got an answer, ms since the epoch. */
  sampledAt: number | null;
  /** The latest request got no answer. */
  unreachable: boolean;
}

export type SyncKey =
  | "Connecting"
  | "Listening"
  | "Bootstrap"
  | "Catchup"
  | "Synced"
  | "NotSynced"
  | "Starting"
  | "NoAnswer";

export interface SyncView {
  key: SyncKey;
  /** Short: the status chip. */
  label: string;
  tone: Tone;
  /** A sentence for the overview. */
  headline: string;
  detail: string;
  tipAgeMs: number | null;
  sampleAgeMs: number | null;
  /** No answer for STATUS_STALE_MS: the numbers on the page are old. */
  stale: boolean;
  /** Highest peer tip minus ours; null when either is unknown. */
  peerDelta: number | null;
}

function age(now: number, at: number | null): number | null {
  return at === null ? null : Math.max(0, now - at);
}

/** How the highest peer tip compares with ours, in words. */
export function peerNote(delta: number | null): string {
  if (delta === null) return "no peer tip to compare";
  if (delta === 0) return "level with ours";
  const n = Math.abs(delta);
  return delta > 0
    ? `${plural(n, "block")} above ours`
    : `${plural(n, "block")} below ours`;
}

export function syncView(input: SyncInput, now: number): SyncView {
  const tipAgeMs = age(now, input.bestTipTimestampMs);
  const sampleAgeMs = age(now, input.sampledAt);
  const stale =
    input.unreachable ||
    (sampleAgeMs !== null && sampleAgeMs > STATUS_STALE_MS);
  const peerDelta =
    input.highestPeerTip !== null && input.bestTipHeight !== null
      ? input.highestPeerTip - input.bestTipHeight
      : null;
  const base = { tipAgeMs, sampleAgeMs, stale, peerDelta };
  const target = formatHeight(input.targetHeight);
  const tip = formatHeight(input.bestTipHeight);

  if (input.phase === null || (input.unreachable && input.sampledAt === null)) {
    return {
      ...base,
      key: "NoAnswer",
      label: "No answer",
      tone: "error",
      headline: "The node isn't answering",
      detail: "No answer from it yet. Is it running?",
    };
  }
  if (input.unreachable) {
    return {
      ...base,
      key: "NoAnswer",
      label: "No answer",
      tone: "error",
      headline: "The node isn't answering",
      detail: `Its last answer came ${formatAge(sampleAgeMs)} ago, at ${tip}. The figures below are from then.`,
    };
  }

  switch (input.phase) {
    case "Connecting":
      return {
        ...base,
        key: "Connecting",
        label: "Connecting",
        tone: "info",
        headline: "Looking for peers",
        detail: "No peer has connected yet.",
      };
    case "Listening":
      return {
        ...base,
        key: "Listening",
        label: "Waiting for peers",
        tone: "info",
        headline: "Connected, waiting for the network",
        detail: "Waiting for a usable best tip from peers.",
      };
    case "Bootstrap":
      return {
        ...base,
        key: "Bootstrap",
        label: "Bootstrapping",
        tone: "info",
        headline:
          input.targetHeight !== null
            ? `Bootstrapping to ${target}`
            : "Bootstrapping",
        detail: "Fetching the ledgers and blocks it needs to follow the chain.",
      };
    case "Catchup": {
      const behind =
        input.targetHeight !== null && input.bestTipHeight !== null
          ? input.targetHeight - input.bestTipHeight
          : null;
      return {
        ...base,
        key: "Catchup",
        label: "Catching up",
        tone: "info",
        headline:
          input.targetHeight !== null
            ? `Catching up to ${target}`
            : "Catching up",
        detail:
          behind !== null && behind > 0 && behind <= K
            ? `${plural(behind, "block")} to apply.`
            : behind !== null && behind > K
              ? "Fetching the ledgers and blocks it needs from its peers."
              : "Applying blocks from its peers.",
      };
    }
    case "Synced": {
      const tooOld = tipAgeMs !== null && tipAgeMs > SYNCED_TIP_MAX_AGE_MS;
      const behind = peerDelta !== null && peerDelta > SYNCED_MAX_BEHIND_PEERS;
      if (tooOld || behind || (tipAgeMs === null && peerDelta === null)) {
        return {
          ...base,
          key: "NotSynced",
          label: "Not synced",
          tone: "warn",
          headline: `Says synced, but ${tip} can't be the chain's tip`,
          detail: tooOld
            ? `Its best tip is ${formatAge(tipAgeMs)} old.`
            : behind
              ? `Peers are at ${formatHeight(input.highestPeerTip)}, ${plural(peerDelta, "block")} ahead.`
              : "It reports no block time and no peer tip to check against.",
        };
      }
      const quiet = tipAgeMs !== null && tipAgeMs > TIP_QUIET_MS;
      return {
        ...base,
        key: "Synced",
        label: "Synced",
        tone: quiet ? "warn" : "ok",
        headline: `In sync at ${tip}`,
        detail: quiet
          ? `No new block for ${formatAge(tipAgeMs)}; the network may be quiet.`
          : `Its newest block is ${formatAge(tipAgeMs)} old${
              peerDelta === null
                ? "."
                : peerDelta === 0
                  ? ", and no peer is ahead of it."
                  : `; the highest peer tip is ${peerNote(peerDelta)}.`
            }`,
      };
    }
    default:
      return {
        ...base,
        key: "Starting",
        label: "Starting",
        tone: "neutral",
        headline: "Starting up",
        detail: "The node hasn't reported a sync phase yet.",
      };
  }
}
