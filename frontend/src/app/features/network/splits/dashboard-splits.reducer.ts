import { AppActions } from '@app/app.actions';
import { isMobile, sort, SortDirection, TableSort } from '@mina-rust/shared';
import { DashboardSplitsState } from '@network/splits/dashboard-splits.state';
import {
  DASHBOARD_SPLITS_CLOSE,
  DASHBOARD_SPLITS_GET_SPLITS,
  DASHBOARD_SPLITS_GET_SPLITS_SUCCESS,
  DASHBOARD_SPLITS_SET_ACTIVE_PEER,
  DASHBOARD_SPLITS_SORT_PEERS,
  DASHBOARD_SPLITS_TOGGLE_SIDE_PANEL,
  DashboardSplitsActions,
} from '@network/splits/dashboard-splits.actions';
import { DashboardSplitsLink } from '@shared/types/network/splits/dashboard-splits-link.type';
import { DashboardSplitsPeer } from '@shared/types/network/splits/dashboard-splits-peer.type';
import { DashboardSplitsSet } from '@shared/types/network/splits/dashboard-splits-set.type';

const initialState: DashboardSplitsState = {
  peers: [],
  links: [],
  sets: [],
  activePeer: undefined,
  fetching: true,
  sort: {
    sortBy: 'outgoingConnections',
    sortDirection: SortDirection.DSC,
  },
  openSidePanel: !isMobile(),
};

export function topologyReducer(
  state: DashboardSplitsState = initialState,
  action:
    | DashboardSplitsActions
    | ReturnType<typeof AppActions.changeActiveNode>,
): DashboardSplitsState {
  switch (action.type) {
    case AppActions.changeActiveNode.type:
      return {
        ...initialState,
        sort: state.sort,
        openSidePanel: state.openSidePanel,
        node: action.node.name,
      };

    case DASHBOARD_SPLITS_GET_SPLITS: {
      return {
        ...state,
        peers: [],
        links: [],
        sets: [],
        activePeer: undefined,
        fetching: true,
      };
    }

    case DASHBOARD_SPLITS_GET_SPLITS_SUCCESS: {
      if (state.node !== undefined && action.payload.node !== state.node) {
        return state;
      }
      const peers = action.payload.peers.map(p => ({
        ...p,
        radius: getRadius(p, action.payload.links),
        outgoingConnections: action.payload.links.filter(
          l => l.source === p.address,
        ).length,
        incomingConnections: action.payload.links.filter(
          l => l.target === p.address,
        ).length,
      }));
      const sets = splitThePeers(peers, action.payload.links);
      return {
        ...state,
        peers,
        node: action.payload.node,
        sets: sets.map(set => ({
          ...set,
          peers: sortPeers(set.peers, state.sort),
        })),
        links: action.payload.links,
        fetching: false,
      };
    }

    case DASHBOARD_SPLITS_SET_ACTIVE_PEER: {
      return {
        ...state,
        activePeer: action.payload,
        openSidePanel: true,
      };
    }

    case DASHBOARD_SPLITS_SORT_PEERS: {
      return {
        ...state,
        sort: action.payload,
        sets: state.sets.map(set => ({
          ...set,
          peers: sortPeers(set.peers, action.payload),
        })),
      };
    }

    case DASHBOARD_SPLITS_TOGGLE_SIDE_PANEL: {
      return {
        ...state,
        openSidePanel: !state.openSidePanel,
      };
    }

    case DASHBOARD_SPLITS_CLOSE:
      return initialState;

    default:
      return state;
  }
}

function getRadius(
  peer: DashboardSplitsPeer,
  links: DashboardSplitsLink[],
): number {
  if (peer.self) {
    return 14;
  }
  const occurrence = links.filter(
    link => link.source === peer.address || link.target === peer.address,
  ).length;
  if (occurrence < 6) {
    return 4;
  } else if (occurrence < 8) {
    return 6;
  } else if (occurrence < 10) {
    return 8;
  } else if (occurrence < 12) {
    return 10;
  } else if (occurrence < 14) {
    return 12;
  } else {
    return 14;
  }
}

function splitThePeers(
  peers: DashboardSplitsPeer[],
  links: DashboardSplitsLink[],
): DashboardSplitsSet[] {
  const sets: DashboardSplitsSet[] = [];
  const visited = new Set<string>();

  for (const peer of peers) {
    if (!visited.has(peer.address)) {
      const peersForCurrentSet = new Set<DashboardSplitsPeer>();
      const queue = [peer];

      while (queue.length > 0) {
        const currentPeer = queue.shift()!;

        if (!peersForCurrentSet.has(currentPeer)) {
          peersForCurrentSet.add(currentPeer);
          visited.add(currentPeer.address);

          for (const link of links) {
            if (
              link.source === currentPeer.address &&
              !visited.has(link.target)
            ) {
              queue.push(peers.find(p => p.address === link.target));
            } else if (
              link.target === currentPeer.address &&
              !visited.has(link.source)
            ) {
              queue.push(peers.find(p => p.address === link.source));
            }
          }
        }
      }

      sets.push({
        peers: sortPeers(Array.from(peersForCurrentSet), {
          sortBy: 'outgoingConnections',
          sortDirection: SortDirection.DSC,
        }),
      });
    }
  }

  return sets;
}

function sortPeers(
  messages: DashboardSplitsPeer[],
  tableSort: TableSort<DashboardSplitsPeer>,
): DashboardSplitsPeer[] {
  return sort<DashboardSplitsPeer>(
    messages,
    tableSort,
    ['address', 'node'],
    true,
  );
}
