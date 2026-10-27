import { MinaNetwork } from '@shared/types/core/mina/mina.type';
import { BlockProductionWonSlotsStatus } from '@shared/types/block-production/won-slots/block-production-won-slots-slot.type';

export interface AppNodeDetails {
  status: AppNodeStatus;
  blockHeight: number;
  /** The node's last sync event (`transition_frontier.sync.time`), in ns.
   * Not the block's age: see bestTipTimestamp. */
  blockTime: number;

  /** Freshness (R26c), all in ms since the epoch. */
  bestTipHash?: string | null;
  /** The best tip block's own timestamp. */
  bestTipTimestamp?: number | null;
  /** When the node's sync state last changed. */
  syncTime?: number | null;
  /** The best tip the node is syncing to, while not synced. */
  targetHeight?: number | null;
  /** When this page last got a status answer. */
  sampledAt?: number | null;
  /** The highest best tip a connected peer reports. */
  maxPeerHeight?: number | null;

  peersConnected: number;
  peersDisconnected: number;
  peersConnecting: number;

  transactions: number;
  snarks: number;

  producingBlockAt: number;
  producingBlockGlobalSlot: number;
  producingBlockStatus: BlockProductionWonSlotsStatus;
  hasBlockProducer?: boolean;

  chainId?: string;
  network?: MinaNetwork;
}

export enum AppNodeStatus {
  PENDING = 'Pending',
  /** Running, no peer yet (F-dash3, OCaml's CONNECTING). */
  CONNECTING = 'Connecting',
  /** Peers, but none has sent a best tip yet (OCaml's LISTENING). */
  LISTENING = 'Listening',
  BOOTSTRAP = 'Bootstrap',
  CATCHUP = 'Catchup',
  SYNCED = 'Synced',
  OFFLINE = 'Offline',
}
