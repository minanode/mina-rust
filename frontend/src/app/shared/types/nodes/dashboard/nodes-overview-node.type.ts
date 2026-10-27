import { NodesOverviewLedger } from '@shared/types/nodes/dashboard/nodes-overview-ledger.type';
import { NodesOverviewBlock } from '@shared/types/nodes/dashboard/nodes-overview-block.type';
import { RestoreSummary } from '@shared/helpers/restore.helper';

export interface NodesOverviewNode {
  name: string;
  kind: NodesOverviewNodeKindType;
  snarks: number;
  transactions: number;
  bestTipReceived: string;
  bestTipReceivedTimestamp: number;
  bestTip: string;
  height: number;
  globalSlot: number;
  appliedBlocks: number;
  applyingBlocks: number;
  missingBlocks: number;
  fetchingBlocks: number;
  fetchedBlocks: number;
  ledgers: NodesOverviewLedger;
  blocks: NodesOverviewBlock[];
  /** The node restarted from saved state rather than bootstrapping. */
  restored?: RestoreSummary | null;
}

export enum NodesOverviewNodeKindType {
  BOOTSTRAP = 'Bootstrap',
  CATCHUP = 'Catchup',
  SYNCED = 'Synced',
  PENDING = 'Pending',
  OFFLINE = 'Offline',
}
