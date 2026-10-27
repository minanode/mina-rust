import type {
  AppNodeDetails,
  AppNodeStatus,
} from '../types/app/app-node-details.type';

/** One source for the public menu and route registration. No operator routes. */
import type { FeaturesConfig } from '../types/core/environment/mina-env.type';

export const PUBLIC_FEATURES: FeaturesConfig = { blocks: [], accounts: [] };
export const PUBLIC_ROUTES = ['blocks', 'blocks/:stateHash', 'accounts'];

export interface PublicNodeStatus {
  chain_id: string | null;
  transition_frontier: {
    best_tip: { hash: string; height: number; timestamp: number | null } | null;
    sync: {
      phase: 'Bootstrap' | 'Catchup' | 'Synced' | 'Pending';
      time: number | null;
    };
  };
}

/** Missing operator measurements stay unknown, rather than becoming zero. */
export function publicNodeDetails(
  data: PublicNodeStatus,
  now: number,
): AppNodeDetails {
  const tip = data.transition_frontier.best_tip;
  const sync = data.transition_frontier.sync;
  return {
    status: sync.phase as AppNodeStatus,
    blockHeight: tip?.height ?? null,
    blockTime: sync.time,
    bestTipHash: tip?.hash ?? null,
    bestTipTimestamp: tip?.timestamp == null ? null : tip.timestamp / 1_000_000,
    syncTime: sync.time == null ? null : sync.time / 1_000_000,
    sampledAt: now,
    chainId: data.chain_id,
    maxPeerHeight: null,
    targetHeight: null,
    peersConnected: null,
    peersConnecting: null,
    peersDisconnected: null,
    transactions: null,
    snarks: null,
    producingBlockAt: null,
    producingBlockGlobalSlot: null,
    producingBlockStatus: null,
  };
}
