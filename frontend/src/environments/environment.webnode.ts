import {
  MinaEnv,
  MinaNode,
} from '@shared/types/core/environment/mina-env.type';
import { BUILD_FRONTEND_CONTROLS } from '@shared/constants/build';

/** Substituted on launch of docker container from MINA_WEBNODE_SEED_URLS by `ng build` */
declare const MINA_WEBNODE_SEED_URLS: string;

/** Default list of seed URLs if MINA_WEBNODE_SEED_URLS isn't specified */
const defaultWebNodeSeedUrls: Readonly<string[]> = [
  'https://bootnodes.minaprotocol.com/networks/devnet-webrtc.txt',
  '/webnode/pkg/devnet-webrtc.txt',
];

/** Substituted on launch of docker container from MINA_WEBNODE_BOOTNODES by `ng build` */
declare const MINA_WEBNODE_BOOTNODES: string;

/** Default list of bootnodes if MINA_WEBNODE_BOOTNODES is unspecified */
const defaultWebNodeBootNodes: Readonly<string[]> = [
  // example:
  // "/2az589QvS6i3EJiVKUfVHCkyqf4khGy9PjQF7nSQuveF27wp7xX/https/mina-rust-seed-1.gcp.o1test.net/443",
];

/** Substituted on launch of docker container from MINA_WEBNODE_NETWORK by `ng build` */
declare const MINA_WEBNODE_NETWORK: string;

/** Substituted on launch of docker container from MINA_WEBNODE_GENESIS_URL by `ng build` */
declare const MINA_WEBNODE_GENESIS_URL: string;

/**
 * Substituted by `ng build --define` from MINA_NATIVE_NODE_URL, as
 * tools/regression/webnode/build.sh does: the HTTP API of a native node the
 * page offers next to the web node, for its block explorer (R28). Empty for
 * none. `typeof` keeps a build without the define working.
 */
declare const MINA_NATIVE_NODE_URL: string;
const nativeNodeUrl: string =
  typeof MINA_NATIVE_NODE_URL === 'string' ? MINA_NATIVE_NODE_URL.trim() : '';
const network: string = (MINA_WEBNODE_NETWORK ?? '').trim() || 'devnet';

/**
 * Substituted by `ng build --define` from MINA_WEBNODE_FRONTIER_STORE (`on` or
 * `off`), as tools/regression/webnode/build.sh does: whether the web node
 * keeps its verified ledgers and blocks in the browser's Origin Private File
 * System and restores from them on a reload (I5e). Off without the define.
 */
declare const MINA_WEBNODE_FRONTIER_STORE: string;
const frontierStore: boolean =
  typeof MINA_WEBNODE_FRONTIER_STORE === 'string' &&
  MINA_WEBNODE_FRONTIER_STORE.trim() === 'on';

/**
 * Substituted by `ng build --define` from MINA_WEBNODE_ICE_SERVERS_URL, as
 * tools/regression/webnode/build.sh does: where the web node fetches its
 * STUN and TURN servers at start (W1). Empty for the node's built-in STUN
 * servers.
 */
declare const MINA_WEBNODE_ICE_SERVERS_URL: string;
const iceServersUrl: string =
  typeof MINA_WEBNODE_ICE_SERVERS_URL === 'string'
    ? MINA_WEBNODE_ICE_SERVERS_URL.trim()
    : '';

/**
 * Substituted by `ng build --define` from MINA_WEBNODE_WASM_URL and
 * MINA_WEBNODE_WASM_SHA256, as tools/regression/webnode/build.sh does when
 * the WASM is on another host (WASM_URL): its URL and SHA-256, which the page
 * checks before it runs it (W1). Empty: the WASM next to the page, unchecked
 * as before.
 */
declare const MINA_WEBNODE_WASM_URL: string;
declare const MINA_WEBNODE_WASM_SHA256: string;
const wasmUrl: string =
  typeof MINA_WEBNODE_WASM_URL === 'string' ? MINA_WEBNODE_WASM_URL.trim() : '';
const wasmSha256: string =
  typeof MINA_WEBNODE_WASM_SHA256 === 'string'
    ? MINA_WEBNODE_WASM_SHA256.trim()
    : '';

/**
 * The native node's pages in the demo: the explorer (blocks and accounts),
 * its dashboard, mempool, sync statistics and the peers it is connected to.
 */
const nativeNode: MinaNode = {
  name: `Native ${network} node`,
  url: nativeNodeUrl,
  features: {
    dashboard: [],
    blocks: [],
    accounts: [],
    mempool: [],
    nodes: ['overview', 'live', 'bootstrap'],
    network: ['topology'],
  },
};

function commaSeparatedEnv(
  envVal: string,
  fallback: Readonly<string[]>,
): Readonly<string[]> {
  const envTrimmed = envVal.trim();
  if (envTrimmed.length !== 0) {
    const envValue = envTrimmed.split(',').map(s => s.trim());
    return envValue;
  }

  return fallback;
}

/**
 * View-only unless the build enables controls with MINA_FRONTEND_CONTROLS
 * (`producer` for block production and key upload, `send`, or `all`).
 */
export const environment: Readonly<MinaEnv> = {
  production: true,
  identifier: 'Web Node FE',
  controls: BUILD_FRONTEND_CONTROLS,
  canAddNodes: true,
  showWebNodeLandingPage: false,
  hidePeersPill: true,
  hideTxPill: true,
  globalConfig: {
    features: {
      dashboard: [],
      state: ['actions'],
      'block-production': ['won-slots'],
      mempool: [],
      benchmarks: ['wallets'],
    },
    webNodeSeedUrls: commaSeparatedEnv(
      MINA_WEBNODE_SEED_URLS ?? '',
      defaultWebNodeSeedUrls,
    ),
    webNodeBootNodes: commaSeparatedEnv(
      MINA_WEBNODE_BOOTNODES ?? '',
      defaultWebNodeBootNodes,
    ),
    webNodeNetwork: (MINA_WEBNODE_NETWORK ?? '').trim() || undefined,
    webNodeGenesisUrl: (MINA_WEBNODE_GENESIS_URL ?? '').trim() || undefined,
    webNodeFrontierStore: frontierStore,
    webNodeIceServersUrl: iceServersUrl || undefined,
    webNodeWasmUrl: wasmUrl || undefined,
    webNodeWasmSha256: wasmSha256 || undefined,
  },
  configs: [
    {
      name: 'Web Node',
      isWebNode: true,
    },
    ...(nativeNodeUrl ? [nativeNode] : []),
  ],
};
