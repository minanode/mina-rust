/**
 * The web node a build of this app starts: read from MINAWEB_* variables by
 * vite.config.ts and compiled into the page as `__MINAWEB_CONFIG__`. The
 * page holds the WASM's SHA-256, so bytes from anywhere else are refused.
 *
 *   MINAWEB_NETWORK         devnet (default) or mainnet
 *   MINAWEB_RELAYS          comma-separated WebRTC multiaddrs the node dials
 *   MINAWEB_SEED_URLS       comma-separated peer lists; default
 *                           /webnode-<network>/peers.txt
 *   MINAWEB_GENESIS_URL     default /webnode-<network>/genesis.json
 *   MINAWEB_ICE_SERVERS_URL STUN and TURN servers, a JSON list; none: the
 *                           node's built-in STUN servers
 *   MINAWEB_FRONTIER_STORE  on or off (default on for devnet)
 *   MINAWEB_GLUE_URL        default /assets/webnode/pkg/mina_node_web.js
 *   MINAWEB_WASM_URL        the WASM, required
 *   MINAWEB_WASM_SHA256     its SHA-256, required
 *   MINAWEB_WASM_BYTES      its size, for the download's progress
 *   MINAWEB_COMMIT          the commit the build was made from
 *
 * With a build-info.json (MINAWEB_ASSETS in development) the WASM, network,
 * relay and frontier store default to that build's.
 */

export interface BuildConfig {
  network: "devnet" | "mainnet";
  relays: string[];
  seedUrls: string[];
  genesisUrl: string;
  iceServersUrl: string | null;
  frontierStore: boolean;
  glueUrl: string;
  wasm: { url: string; sha256: string; bytes: number | null };
  commit: string | null;
}

type Env = Record<string, string | undefined>;

const list = (value: string | undefined) =>
  (value ?? "")
    .split(",")
    .map(s => s.trim())
    .filter(Boolean);

const RELAY =
  /^\/(ip4|ip6|dns4|dns6|dns)\/[^/]+\/tcp\/\d+\/webrtc\/https?\/p2p\/\w+$/;

export function readBuildConfig(
  env: Env,
  info: Record<string, unknown> | null = null,
): BuildConfig {
  const fromInfo = (key: string) =>
    typeof info?.[key] === "string" ? (info[key] as string) : undefined;
  const infoWasm = (info?.wasm ?? null) as Record<string, unknown> | null;

  const network = (env.MINAWEB_NETWORK || fromInfo("network") || "devnet") as
    | string
    | undefined;
  if (network !== "devnet" && network !== "mainnet") {
    throw new Error(`MINAWEB_NETWORK is devnet or mainnet, not ${network}`);
  }
  const relays = list(env.MINAWEB_RELAYS ?? fromInfo("relay"));
  for (const relay of relays) {
    if (!RELAY.test(relay)) {
      throw new Error(`MINAWEB_RELAYS: not a WebRTC multiaddr: ${relay}`);
    }
  }
  const store =
    env.MINAWEB_FRONTIER_STORE ??
    (typeof info?.frontierStore === "boolean"
      ? info.frontierStore
        ? "on"
        : "off"
      : network === "devnet"
        ? "on"
        : "off");
  if (store !== "on" && store !== "off") {
    throw new Error(`MINAWEB_FRONTIER_STORE is on or off, not ${store}`);
  }
  const wasmUrl =
    env.MINAWEB_WASM_URL ||
    (typeof infoWasm?.path === "string" ? infoWasm.path : "");
  const sha256 = (
    env.MINAWEB_WASM_SHA256 ||
    (typeof infoWasm?.sha256 === "string" ? infoWasm.sha256 : "")
  ).toLowerCase();
  const bytesText =
    env.MINAWEB_WASM_BYTES ||
    (typeof infoWasm?.bytes === "number" ? String(infoWasm.bytes) : "");
  if (!wasmUrl || !/^[0-9a-f]{64}$/.test(sha256)) {
    throw new Error(
      "set MINAWEB_WASM_URL and MINAWEB_WASM_SHA256 (or MINAWEB_ASSETS): the page checks the WASM before it runs it",
    );
  }
  const bytes = /^\d+$/.test(bytesText) ? Number(bytesText) : null;
  const seedUrls = list(env.MINAWEB_SEED_URLS);
  return {
    network,
    relays,
    seedUrls: seedUrls.length ? seedUrls : [`/webnode-${network}/peers.txt`],
    genesisUrl: env.MINAWEB_GENESIS_URL || `/webnode-${network}/genesis.json`,
    iceServersUrl:
      env.MINAWEB_ICE_SERVERS_URL ||
      (typeof info?.iceServers === "string" ? info.iceServers : null),
    frontierStore: store === "on",
    glueUrl: env.MINAWEB_GLUE_URL || "/assets/webnode/pkg/mina_node_web.js",
    wasm: { url: wasmUrl, sha256, bytes },
    commit: env.MINAWEB_COMMIT || fromInfo("stamp") || null,
  };
}
