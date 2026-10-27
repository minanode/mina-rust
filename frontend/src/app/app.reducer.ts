import { AppActions } from '@app/app.actions';
import { AppState } from '@app/app.state';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import { createReducer, on } from '@ngrx/store';
import { AppNodeStatus } from '@shared/types/app/app-node-details.type';
import { getLocalStorage } from '@mina-rust/shared';

const initialState: AppState = {
  menu: {
    collapsed:
      JSON.parse(getLocalStorage()?.getItem('menu_collapsed') ?? 'false') ||
      false,
    isMobile: false,
    open: true,
  },
  nodes: [],
  activeNode: undefined,
  activeNodeDetails: {
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
  },
  envBuild: undefined,
};

export const appReducer = createReducer(
  initialState,
  on(AppActions.initSuccess, (state, { activeNode, nodes }) => ({
    ...state,
    activeNode,
    nodes,
  })),
  on(
    AppActions.changeActiveNode,
    (state, { node }): AppState => ({
      ...state,
      activeNode: node,
      activeNodeDetails: initialState.activeNodeDetails,
      // The last node's build must not be shown as this one's (R27 #2).
      envBuild: undefined,
    }),
  ),
  on(AppActions.getNodeDetailsSuccess, (state, { details, node }) =>
    node !== state.activeNode?.name
      ? state
      : {
          ...state,
          activeNodeDetails:
            details.status === AppNodeStatus.OFFLINE
              ? {
                  // No answer: keep what the last answer said, and when it came,
                  // so the page can say how stale it is (R26c).
                  ...details,
                  blockHeight: state.activeNodeDetails.blockHeight,
                  bestTipHash: state.activeNodeDetails.bestTipHash,
                  bestTipTimestamp: state.activeNodeDetails.bestTipTimestamp,
                  syncTime: state.activeNodeDetails.syncTime,
                  sampledAt: state.activeNodeDetails.sampledAt,
                  chainId: state.activeNodeDetails.chainId,
                  network: state.activeNodeDetails.network,
                }
              : details,
        },
  ),
  on(AppActions.changeMenuCollapsing, (state, { isCollapsing }) => {
    getLocalStorage()?.setItem('menu_collapsed', JSON.stringify(isCollapsing));
    return { ...state, menu: { ...state.menu, collapsed: isCollapsing } };
  }),
  on(AppActions.toggleMobile, (state, { isMobile }) => ({
    ...state,
    menu: { ...state.menu, isMobile, open: !isMobile },
  })),
  on(AppActions.toggleMenuOpening, state => ({
    ...state,
    menu: { ...state.menu, open: !state.menu.open },
  })),
  on(AppActions.addNode, (state, { node }) => {
    const customNodes = JSON.parse(
      getLocalStorage()?.getItem('custom_nodes') ?? '[]',
    );
    getLocalStorage()?.setItem(
      'custom_nodes',
      JSON.stringify([node, ...customNodes]),
    );
    return { ...state, nodes: [node, ...state.nodes] };
  }),
  on(AppActions.deleteNode, (state, { node }) => {
    const customNodes = JSON.parse(
      getLocalStorage()?.getItem('custom_nodes') ?? '[]',
    );
    getLocalStorage()?.setItem(
      'custom_nodes',
      JSON.stringify(customNodes.filter((n: MinaNode) => n.name !== node.name)),
    );
    const nodes = state.nodes.filter(n => n.name !== node.name);
    return {
      ...state,
      nodes,
      activeNode:
        state.activeNode?.name === node.name ? nodes[0] : state.activeNode,
    };
  }),
  on(
    AppActions.getNodeEnvBuildSuccess,
    (state, { envBuild, node }): AppState =>
      // A late answer from a node no longer selected is dropped.
      node !== undefined && node !== state.activeNode?.name
        ? state
        : { ...state, envBuild },
  ),
);
