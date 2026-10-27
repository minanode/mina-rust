import { ChangeDetectionStrategy, Component, OnInit } from '@angular/core';
import { StoreDispatcher } from '@shared/base-classes/store-dispatcher.class';
import { selectDashboardNodesAndPeers } from '@dashboard/dashboard.state';
import { NodesOverviewNode } from '@shared/types/nodes/dashboard/nodes-overview-node.type';
import { NodesOverviewNodeBlockStatus } from '@shared/types/nodes/dashboard/nodes-overview-block.type';
import { isDesktop, lastItem, ONE_MILLION } from '@mina-rust/shared';
import { DashboardPeer } from '@shared/types/dashboard/dashboard.peer';
import { AppActions } from '@app/app.actions';
import { formatHeight } from '@shared/helpers/freshness.helper';

const PENDING = 'Pending';

@Component({
  selector: 'mina-dashboard-blocks-sync',
  templateUrl: './dashboard-blocks-sync.component.html',
  styleUrls: ['./dashboard-blocks-sync.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
  standalone: false,
})
export class DashboardBlocksSyncComponent
  extends StoreDispatcher
  implements OnInit
{
  fetched: number;
  fetchedPercentage: string = '-';
  applied: number;
  appliedPercentage: number;
  root: number;
  rootText: string = PENDING;
  bestTipBlock: number;
  bestTipBlockSyncedText: string = PENDING;
  targetBlock: number;
  syncProgress: string;
  isDesktop: boolean = isDesktop();
  remaining: number;
  lengthWithoutRoot: number = null;
  readonly formatHeight = formatHeight;

  constructor() {
    super();
  }

  ngOnInit(): void {
    this.listenToNodesChanges();
  }

  private listenToNodesChanges(): void {
    this.select(
      selectDashboardNodesAndPeers,
      ([nodes, peers]: [NodesOverviewNode[], DashboardPeer[]]) => {
        if (nodes.length === 0) {
          this.fetched = undefined;
          this.fetchedPercentage = '-';
          this.applied = undefined;
          this.appliedPercentage = undefined;
          this.root = undefined;
          this.rootText = PENDING;
          this.bestTipBlock = undefined;
          this.bestTipBlockSyncedText = PENDING;
          this.targetBlock = undefined;
          this.syncProgress = undefined;
        } else {
          const blocks = nodes[0].blocks;

          const blocksFetched = blocks.filter(b => b.fetchEnd).length;
          const blocksApplied = blocks.filter(b => b.applyEnd).length;
          if (blocksApplied < 291) {
            const syncStart =
              Math.min(...blocks.map(b => b.fetchStart).filter(Boolean)) /
              ONE_MILLION;
            const now = Date.now();
            const secondsPassed = (now - syncStart) / 1000;

            const fetchWeight = 1;
            const applyWeight = 5;

            // Apply weights: 1 for each fetched block, and 5 for each applied block
            const weightedBlocksTotal =
              blocksFetched * fetchWeight + blocksApplied * applyWeight;
            const blocksPerSecond = weightedBlocksTotal / secondsPassed;

            if (blocksPerSecond > 0) {
              const weightedBlocksRemaining =
                291 * fetchWeight + 291 * applyWeight - weightedBlocksTotal;
              const secondsRemaining =
                weightedBlocksRemaining / blocksPerSecond;
              this.remaining = Math.ceil(secondsRemaining);
            }
          } else {
            this.remaining = null;
          }

          this.extractNodesData(nodes);
          this.extractPeersData(peers);

          if (this.appliedPercentage === 100) {
            this.dispatch2(AppActions.getNodeDetails());
          }
        }
        this.detect();
      },
    );
  }

  private extractPeersData(peers: DashboardPeer[]): void {
    const highestHeightPeer = peers.reduce(
      (acc: DashboardPeer, peer: DashboardPeer) =>
        peer.height > acc.height ? peer : acc,
      { height: 0 } as DashboardPeer,
    );
    this.targetBlock = highestHeightPeer.height;
  }

  private extractNodesData(nodes: NodesOverviewNode[]): void {
    let blocks = nodes[0].blocks;

    if (blocks.length > 0) {
      this.bestTipBlock = blocks[0].height;
      // When the best tip arrived (or was applied, below), not when it was
      // made: the block's own age is in the freshness panel.
      const received = this.calculateProgressTime(
        nodes[0].bestTipReceivedTimestamp * ONE_MILLION,
      );
      this.bestTipBlockSyncedText = this.cardHint('Received', received);
      this.syncProgress = received;
      if (lastItem(blocks).status !== NodesOverviewNodeBlockStatus.APPLIED) {
        this.syncProgress = PENDING;
      }
    }

    // k + 1 blocks from the best tip back to the root. A restore from saved
    // state replays a few more, from an older stored root (F-dash3).
    if (blocks.length >= 291) {
      const root = blocks[290];
      this.root = root.height;
      this.rootText = root.applyEnd
        ? 'Applied ' + this.calculateProgressTime(root.applyEnd)
        : PENDING;
      if (
        blocks[0].status === NodesOverviewNodeBlockStatus.APPLIED &&
        blocks[0].applyEnd
      ) {
        this.bestTipBlockSyncedText = this.cardHint(
          'Applied',
          this.calculateProgressTime(blocks[0].applyEnd),
        );
      }
    } else {
      this.root = null;
      this.rootText = PENDING;
    }
    // blocks can be 292
    blocks = blocks.slice(1);

    this.lengthWithoutRoot = blocks.length ?? null; // 290 or 291

    this.fetched = blocks.filter(
      b =>
        ![
          NodesOverviewNodeBlockStatus.MISSING,
          NodesOverviewNodeBlockStatus.FETCHING,
        ].includes(b.status),
    ).length;
    this.applied = blocks.filter(
      b => b.status === NodesOverviewNodeBlockStatus.APPLIED,
    ).length;
    this.fetchedPercentage =
      Math.floor((this.fetched * 100) / (this.lengthWithoutRoot || 100)) + '%';
    this.appliedPercentage = Math.floor(
      (this.applied * 100) / (this.lengthWithoutRoot || 100),
    );
  }

  /** "Applied 3m ago"; the age alone on a phone, where the card is narrow. */
  private cardHint(event: string, age: string): string {
    return this.isDesktop && age !== PENDING ? `${event} ${age}` : age;
  }

  /** "3m ago", or Pending without a time. Says nothing about sync. */
  private calculateProgressTime(timestamp: number): string {
    if (!timestamp) {
      return PENDING;
    }
    timestamp = Math.ceil(timestamp / ONE_MILLION);
    const millisecondsAgo = Date.now() - timestamp;
    const minutesAgo = Math.floor(millisecondsAgo / 1000 / 60);
    const hoursAgo = Math.floor(minutesAgo / 60);
    const daysAgo = Math.floor(hoursAgo / 24);

    if (daysAgo > 0) {
      return `${daysAgo}d ago`;
    } else if (hoursAgo > 0) {
      return `${hoursAgo}h ago`;
    } else if (minutesAgo > 0) {
      return `${minutesAgo}m ago`;
    } else {
      return `<1m ago`;
    }
  }
}
