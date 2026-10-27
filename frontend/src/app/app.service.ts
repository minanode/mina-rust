import { Injectable } from '@angular/core';
import { map, Observable, of, take, tap, timeout } from 'rxjs';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import {
  CONFIG,
  nodeSupportsPage,
  withoutDisabledControls,
} from '@shared/constants/config';
import { initialNode } from '@shared/helpers/node-selection.helper';
import { RustService } from '@core/services/rust.service';
import {
  AppNodeDetails,
  AppNodeStatus,
} from '@shared/types/app/app-node-details.type';
import { getNetwork } from '@shared/helpers/mina.helper';
import { getLocalStorage, nanOrElse, ONE_MILLION } from '@mina-rust/shared';
import { BlockProductionWonSlotsStatus } from '@shared/types/block-production/won-slots/block-production-won-slots-slot.type';
import { AppEnvBuild } from '@shared/types/app/app-env-build.type';
import { WebNodeService } from '@core/services/web-node.service';
import {
  PublicNodeStatus,
  publicNodeDetails,
} from '@shared/helpers/public-api';
import { nanosToMs } from '@shared/helpers/freshness.helper';

/** A status answer later than this counts as none: the node is shown offline. */
const STATUS_TIMEOUT_MS = 20000;

@Injectable({
  providedIn: 'root',
})
export class AppService {
  private previousProducedBlock: BlockProductionAttempt;

  constructor(
    private rust: RustService,
    private webNode: WebNodeService,
  ) {}

  getActiveNode(nodes: MinaNode[]): Observable<MinaNode> {
    return of(this.initialNode(nodes));
  }

  /**
   * The node a page load starts on: the URL's `node`, else one that can show
   * the page (a native node for /blocks), else the first (R28).
   */
  initialNode(nodes: MinaNode[] = this.nodes()): MinaNode {
    return initialNode(nodes, location.href, nodeSupportsPage);
  }

  getNodes(): Observable<MinaNode[]> {
    return of(this.nodes());
  }

  /** The configured nodes, then the ones added in this browser. */
  nodes(): MinaNode[] {
    if (CONFIG.publicApi) return CONFIG.configs;
    const custom: MinaNode[] = JSON.parse(
      getLocalStorage()?.getItem('custom_nodes') ?? '[]',
    );
    return [
      ...CONFIG.configs,
      ...custom.map(node => ({
        ...node,
        features: withoutDisabledControls(node.features),
      })),
    ];
  }

  getEnvBuild(): Observable<AppEnvBuild> {
    return CONFIG.publicApi
      ? of(undefined)
      : this.rust.get<AppEnvBuild>('/build_env');
  }

  getActiveNodeDetails(node?: MinaNode): Observable<AppNodeDetails> {
    if (CONFIG.publicApi) {
      return this.rust.get<PublicNodeStatus>('/status').pipe(
        take(1),
        timeout(STATUS_TIMEOUT_MS),
        map(data => ({
          ...publicNodeDetails(data, Date.now()),
          network: getNetwork(data.chain_id),
        })),
      );
    }
    if (
      (node?.isWebNode ?? this.rust.activeNodeIsWebNode) &&
      !this.webNode.started
    ) {
      // Still loading: starting, not offline.
      return of(STARTING);
    }
    return this.rust.get<NodeDetailsResponse>('/status', node).pipe(
      // A request that hangs must not leave the last answer looking fresh.
      take(1),
      timeout(STATUS_TIMEOUT_MS),
      tap((data: NodeDetailsResponse) => {
        this.previousProducedBlock = data.previous_block_production_attempt;
      }),
      map(
        (data: NodeDetailsResponse): AppNodeDetails =>
          ({
            status: this.getStatus(data),
            blockHeight: data.transition_frontier.best_tip?.height,
            blockTime: data.transition_frontier.sync.time,
            ...freshnessFields(data, Date.now()),
            peersConnected: data.peers.filter(
              p => p.connection_status === 'Connected',
            ).length,
            peersDisconnected: data.peers.filter(
              p => p.connection_status === 'Disconnected',
            ).length,
            peersConnecting: data.peers.filter(
              p => p.connection_status === 'Connecting',
            ).length,
            snarks: data.snark_pool.snarks,
            transactions: data.transaction_pool.transactions,
            chainId: data.chain_id,
            network: getNetwork(data.chain_id),
            hasBlockProducer:
              data.block_producer === undefined
                ? undefined
                : data.block_producer !== null,
            producingBlockAt: nanOrElse(
              data.current_block_production_attempt?.won_slot.slot_time /
                ONE_MILLION,
              null,
            ),
            producingBlockGlobalSlot:
              data.current_block_production_attempt?.won_slot.global_slot,
            producingBlockStatus: data.current_block_production_attempt?.status,
          }) as AppNodeDetails,
      ),
    );
  }

  private getStatus(data: NodeDetailsResponse): AppNodeStatus {
    switch (data.transition_frontier.sync.phase) {
      case 'Connecting':
        return AppNodeStatus.CONNECTING;
      case 'Listening':
        return AppNodeStatus.LISTENING;
      case 'Bootstrap':
        return AppNodeStatus.BOOTSTRAP;
      case 'Catchup':
        return AppNodeStatus.CATCHUP;
      case 'Synced':
        return AppNodeStatus.SYNCED;
      default:
        return AppNodeStatus.PENDING;
    }
  }
}

const STARTING: AppNodeDetails = {
  status: AppNodeStatus.PENDING,
  blockHeight: null,
  blockTime: null,
  peersConnected: 0,
  peersDisconnected: 0,
  peersConnecting: 0,
  transactions: 0,
  snarks: 0,
  producingBlockAt: null,
  producingBlockGlobalSlot: null,
  producingBlockStatus: null,
};

/**
 * Block age and freshness (R26c). The best tip's timestamp comes from the
 * node (after m6); from an older node, from a peer whose best tip is the
 * same block, else it stays unknown.
 */
export function freshnessFields(
  data: NodeDetailsResponse,
  now: number,
): Partial<AppNodeDetails> {
  const tip = data.transition_frontier.best_tip;
  const connected = data.peers.filter(p => p.connection_status === 'Connected');
  const peerTimestamp = connected.find(
    p => tip && p.best_tip === tip.hash,
  )?.best_tip_timestamp;
  const heights = connected
    .map(p => p.best_tip_height)
    .filter((h): h is number => typeof h === 'number' && h > 0);
  return {
    bestTipHash: tip?.hash ?? null,
    bestTipTimestamp: nanosToMs(tip?.timestamp ?? peerTimestamp),
    syncTime: nanosToMs(data.transition_frontier.sync.time),
    targetHeight: data.transition_frontier.sync.target?.height ?? null,
    sampledAt: now,
    maxPeerHeight: heights.length ? Math.max(...heights) : null,
  };
}

export interface NodeDetailsResponse {
  block_producer?: string | null;
  transition_frontier: TransitionFrontier;
  transaction_pool: { transactions: number };
  peers: Peer[];
  snark_pool: SnarkPool;
  chain_id: string | undefined;
  current_block_production_attempt: BlockProductionAttempt;
  previous_block_production_attempt: BlockProductionAttempt;
}

export interface BlockProductionAttempt {
  won_slot: WonSlot;
  block: any;
  times: Times;
  status: BlockProductionWonSlotsStatus;
}

export interface WonSlot {
  slot_time: number;
  global_slot: number;
  epoch: number;
  delegator: [string, number];
  value_with_threshold: number[];
}

export interface Times {
  scheduled: number;
  staged_ledger_diff_create_start: any;
  staged_ledger_diff_create_end: any;
  produced: any;
  proof_create_start: any;
  proof_create_end: any;
  block_apply_start: any;
  block_apply_end: any;
  committed: any;
  discarded: any;
}

interface TransitionFrontier {
  best_tip: BestTip;
  sync: Sync;
}

interface BestTip {
  hash: string;
  height: number;
  global_slot: number;
  /** The block's timestamp in ns; from nodes after m6. */
  timestamp?: number | null;
}

interface Sync {
  time: number;
  status: string;
  target: BestTip | null;
  phase: 'Connecting' | 'Listening' | 'Bootstrap' | 'Catchup' | 'Synced';
}

interface Peer {
  peer_id: string;
  best_tip: string;
  best_tip_height: number;
  best_tip_global_slot: number;
  best_tip_timestamp: number;
  connection_status: string;
  address: string;
  time: number;
}

interface SnarkPool {
  total_jobs: number;
  snarks: number;
}
