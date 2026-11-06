/**
 * Why the web node can't start or connect, in words: each failure names its
 * real cause and what would fix it. The checks take plain values, so the
 * tests can give them any browser.
 */

export type FailureKind =
  | "isolation"
  | "threads"
  | "files"
  | "download"
  | "integrity"
  | "load"
  | "start"
  | "webrtc"
  | "peers";

export interface BootFailure {
  kind: FailureKind;
  title: string;
  detail: string;
  /** Lines an operator sets as they are (headers): in mono, one a line,
   * under the detail. */
  lines?: string[];
  /** What to do about it, when there is something. */
  hint?: string;
  /** Before Start, what can't run it: the page (how it's served) or, by
   * default, the browser. */
  cause?: "page";
}

export interface PageFacts {
  /** The browser knows cross-origin isolation at all (it has
   * `crossOriginIsolated`): older ones don't, whatever the server sends. */
  knowsIsolation: boolean;
  crossOriginIsolated: boolean;
  isSecureContext: boolean;
  /** `location.origin`. */
  origin: string;
  hasWebAssembly: boolean;
  /** `new WebAssembly.Memory({ shared: true, ... })` works. */
  hasSharedMemory: boolean;
  /** The page can make WebRTC connections (`RTCPeerConnection`). */
  hasWebRtc: boolean;
}

/** The browser and the page can run the node's threads; or why not. */
export function checkPage(page: PageFacts): BootFailure | null {
  if (!page.hasWebAssembly) {
    return {
      kind: "threads",
      title: "WebAssembly is off in this browser",
      detail:
        "The web node is a WebAssembly program, and this browser has no WebAssembly. Some security settings turn it off, such as Lockdown Mode on an iPhone, iPad or Mac.",
      hint: "Exclude this site from the setting, or open the page in another browser.",
    };
  }
  if (!page.isSecureContext) {
    return {
      kind: "isolation",
      cause: "page",
      title: "This page needs HTTPS",
      detail: `The page is open at ${page.origin}. Browsers give the node's threads shared memory only on HTTPS pages, or on localhost.`,
      hint: "Open it over HTTPS, or from localhost.",
    };
  }
  if (!page.knowsIsolation) {
    return {
      kind: "isolation",
      title: "This browser is too old to run the node",
      detail:
        "The node's threads share memory, which browsers allow only on a cross-origin isolated page. This browser can't isolate a page.",
      hint: "Update the browser, or open the page in a current Chrome, Edge, Firefox or Safari.",
    };
  }
  if (!page.crossOriginIsolated) {
    return {
      kind: "isolation",
      cause: "page",
      title: "The server didn't isolate this page",
      detail:
        "The node's threads share memory, which needs both headers. This server didn't send them.",
      lines: [
        "Cross-Origin-Opener-Policy: same-origin",
        "Cross-Origin-Embedder-Policy: require-corp",
      ],
      hint: "Set both, as serve.json in the build does.",
    };
  }
  if (!page.hasSharedMemory) {
    return {
      kind: "threads",
      title: "This browser can't run WebAssembly threads",
      detail:
        "The page is isolated, but the browser refused shared WebAssembly memory. The node needs it for its threads.",
      hint: "Open the page in a current Chrome, Edge, Firefox or Safari.",
    };
  }
  if (!page.hasWebRtc) {
    return {
      kind: "webrtc",
      title: "WebRTC is off in this browser",
      detail:
        "The node reaches its peers over WebRTC data channels, and this browser has no RTCPeerConnection. A setting or an extension may have turned it off.",
      hint: "Turn WebRTC on for this site, or open the page in another browser. In Firefox, the setting is media.peerconnection.enabled in about:config.",
    };
  }
  return null;
}

/** The browser's facts, read now. */
export function pageFacts(): PageFacts {
  let hasSharedMemory = false;
  try {
    hasSharedMemory =
      new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true })
        .buffer instanceof SharedArrayBuffer;
  } catch {
    hasSharedMemory = false;
  }
  return {
    knowsIsolation: "crossOriginIsolated" in globalThis,
    crossOriginIsolated: globalThis.crossOriginIsolated === true,
    isSecureContext: globalThis.isSecureContext === true,
    origin: location.origin,
    hasWebAssembly: typeof WebAssembly === "object",
    hasSharedMemory,
    hasWebRtc: typeof globalThis.RTCPeerConnection === "function",
  };
}

/** `host:port` of a WebRTC multiaddr, and whether it signals over HTTPS. */
export function relayEndpoint(
  relay: string,
): { host: string; secure: boolean } | null {
  const match =
    /^\/(ip4|ip6|dns4|dns6|dns)\/([^/]+)\/tcp\/(\d+)\/webrtc\/(https?)\//.exec(
      relay,
    );
  if (!match) return null;
  const [, family, host, port, scheme] = match;
  return {
    host: family === "ip6" ? `[${host}]:${port}` : `${host}:${port}`,
    secure: scheme === "https",
  };
}

const LOCAL = /^(localhost|127\.\d+\.\d+\.\d+|\[::1\])(:\d+)?$/;

/** A relay the browser won't let this page reach, and why: an HTTPS page
 * can't signal over plain HTTP, except to this machine. */
export function blockedRelay(relay: string, pageOrigin: string): string | null {
  const endpoint = relayEndpoint(relay);
  if (!endpoint || endpoint.secure) return null;
  if (!pageOrigin.startsWith("https:")) return null;
  if (LOCAL.test(endpoint.host)) return null;
  return `${endpoint.host} signals over plain HTTP, which the browser blocks on an HTTPS page`;
}

/** No connected peer after a wait: what the node tried, and what it saw. */
export function noPeerFailure(
  relays: string[],
  pageOrigin: string,
  peers: { address: string | null; status: string }[],
): BootFailure {
  const blocked = relays
    .map(relay => blockedRelay(relay, pageOrigin))
    .filter((reason): reason is string => reason !== null);
  if (blocked.length > 0 && blocked.length === relays.length) {
    return {
      kind: "peers",
      title: "No peer reachable",
      detail: `${blocked.join(". ")}.`,
      hint: "Give the relay an HTTPS signalling address, or open the page over HTTP from localhost.",
    };
  }
  if (relays.length === 0 && peers.length === 0) {
    return {
      kind: "peers",
      title: "No peer to connect to",
      detail: "This build names no relay, and the seed list gave no peers.",
      hint: "Build the page with a relay address.",
    };
  }
  const tried = relays.length
    ? relays.map(relay => {
        const host = relayEndpoint(relay)?.host ?? relay;
        const seen = peers.find(p => p.address === relay);
        // Disconnected goes without saying; another state is news.
        return seen && seen.status !== "Disconnected"
          ? `${host} (${seen.status.toLowerCase()})`
          : host;
      })
    : peers.map(p => `${p.address ?? "a peer"} (${p.status.toLowerCase()})`);
  const several = tried.length > 1;
  return {
    kind: "peers",
    title: "No peer reachable",
    // The step beside it keeps its own clock: no second one here.
    detail: `No connection to ${
      several
        ? `${tried.slice(0, -1).join(", ")} or ${tried[tried.length - 1]}`
        : tried[0]
    }.`,
    hint: `The ${several ? "relays" : "relay"} may be down, or this network blocks WebRTC. Still trying.`,
  };
}

/** Bytes as the page shows sizes: 37.8 MB. */
export function formatBytes(bytes: number | null): string {
  if (bytes === null) return "–";
  if (bytes < 1_000) return `${bytes} B`;
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(0)} kB`;
  if (bytes < 1_000_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${(bytes / 1_000_000_000).toFixed(2)} GB`;
}
