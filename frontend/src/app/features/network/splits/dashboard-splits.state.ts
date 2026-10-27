import { createSelector, MemoizedSelector } from '@ngrx/store';
import { TableSort } from '@mina-rust/shared';
import { DashboardSplitsPeer } from '@shared/types/network/splits/dashboard-splits-peer.type';
import { DashboardSplitsSet } from '@shared/types/network/splits/dashboard-splits-set.type';
import { DashboardSplitsLink } from '@shared/types/network/splits/dashboard-splits-link.type';
import { MinaState } from '@app/app.setup';
import { selectDashboardSplitsState } from '@network/network.state';

export interface DashboardSplitsState {
  node?: string;
  peers: DashboardSplitsPeer[];
  links: DashboardSplitsLink[];
  sets: DashboardSplitsSet[];
  activePeer: DashboardSplitsPeer;
  fetching: boolean;
  sort: TableSort<DashboardSplitsPeer>;
  openSidePanel: boolean;
}

const select = <T>(
  selector: (state: DashboardSplitsState) => T,
): MemoizedSelector<MinaState, T> =>
  createSelector(selectDashboardSplitsState, selector);

export const selectDashboardSplitsNode = select(state => state.node);

export const selectDashboardSplitsPeers = select(
  (state: DashboardSplitsState): DashboardSplitsPeer[] => state.peers,
);
export const selectDashboardSplitsLinks = select(
  (state: DashboardSplitsState): DashboardSplitsLink[] => state.links,
);
export const selectDashboardSplitsSets = select(
  (state: DashboardSplitsState): DashboardSplitsSet[] => state.sets,
);
export const selectDashboardSplitsFetching = select(
  (state: DashboardSplitsState): boolean => state.fetching,
);
export const selectDashboardSplitsPeersAndLinksAndSetsAndFetching =
  createSelector(
    selectDashboardSplitsPeers,
    selectDashboardSplitsLinks,
    selectDashboardSplitsSets,
    selectDashboardSplitsFetching,
    selectDashboardSplitsNode,
    (
      peers: DashboardSplitsPeer[],
      links: DashboardSplitsLink[],
      sets: DashboardSplitsSet[],
      fetching: boolean,
      node: string,
    ) => ({ peers, links, sets, fetching, node }),
  );
export const selectDashboardSplitsPeersAndSets = createSelector(
  selectDashboardSplitsPeers,
  selectDashboardSplitsSets,
  (peers: DashboardSplitsPeer[], sets: DashboardSplitsSet[]) => ({
    peers,
    sets,
  }),
);
export const selectDashboardSplitsActivePeer = select(
  (state: DashboardSplitsState): DashboardSplitsPeer => state.activePeer,
);
export const selectDashboardSplitsSort = select(
  (state: DashboardSplitsState): TableSort<DashboardSplitsPeer> => state.sort,
);
export const selectDashboardSplitsOpenSidePanel = select(
  (state: DashboardSplitsState): boolean => state.openSidePanel,
);
