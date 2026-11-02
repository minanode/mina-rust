/**
 * When stake counts. A key wins slots in epoch N with the stake its
 * staking ledger gives it: the ledger as it was at the end of epoch N-2.
 * At each epoch change the next epoch ledger becomes the staking ledger,
 * and the ledger at the change becomes the next epoch ledger. So stake
 * delegated in a block of epoch N counts from the start of epoch N+2 at
 * the earliest, however much of epoch N is left. Not always then: the node
 * takes the snarked ledger at the change, so a delegation in one of the
 * epoch's last blocks, not yet in a ledger proof, waits an epoch more.
 */

import { winProbability } from "@minanode/node-client";

import type { Consensus, ConsensusTime, Stake } from "./node";

export const DAY_MS = 86_400_000;

export interface EpochClock {
  epoch: number;
  /** When the current epoch ends, ms since the Unix epoch. */
  epochEndMs: number;
  epochMs: number;
  /** Earliest activation, conditional on reaching the snarked ledger. */
  delegationEpoch: number;
  delegationCountsFromMs: number;
}

export function epochClock(
  consensus: Consensus,
  time: ConsensusTime,
): EpochClock {
  const epochMs = consensus.slotsPerEpoch * consensus.slotMs;
  const epochEndMs =
    time.slotStartMs + (consensus.slotsPerEpoch - time.slot) * consensus.slotMs;
  return {
    epoch: time.epoch,
    epochEndMs,
    epochMs,
    delegationEpoch: time.epoch + 2,
    delegationCountsFromMs: epochEndMs + epochMs,
  };
}

/** Days, to one decimal below ten, whole above. */
export function days(ms: number): string {
  const d = Math.max(0, ms) / DAY_MS;
  const text = d < 10 ? d.toFixed(1) : String(Math.round(d));
  return `${text} ${text === "1.0" ? "day" : "days"}`;
}

export type StakeVerdict =
  /** No stake now or next epoch: it will not produce. */
  | { kind: "none" }
  /** Stake in the next epoch ledger only: from the next epoch. */
  | { kind: "next"; fromMs: number; epoch: number }
  /** Stake now: it can win slots this epoch. */
  | { kind: "now"; expectedPerEpoch: number }
  /** The node couldn't read the ledgers yet. */
  | { kind: "unknown" };

export function stakeVerdict(
  stake: Stake | null,
  clock: EpochClock | null,
  consensus: Consensus | null,
): StakeVerdict {
  if (!stake || stake.staking.delegatedStake === null)
    return { kind: "unknown" };
  const now = stake.staking.delegatedStake;
  if (now > 0n) {
    const perSlot = winProbability(
      Number(now),
      Number(stake.staking.totalCurrency),
    );
    return {
      kind: "now",
      expectedPerEpoch: consensus ? perSlot * consensus.slotsPerEpoch : 0,
    };
  }
  const next = stake.next.delegatedStake;
  // No stake now, and the next ledger not read: not known yet.
  if (next === null) return { kind: "unknown" };
  if (next > 0n && clock) {
    return { kind: "next", fromMs: clock.epochEndMs, epoch: stake.epoch + 1 };
  }
  return next > 0n ? { kind: "unknown" } : { kind: "none" };
}
