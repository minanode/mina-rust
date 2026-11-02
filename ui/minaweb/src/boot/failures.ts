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
  | "peers";

export interface BootFailure {
  kind: FailureKind;
  title: string;
  detail: string;
  /** What to do about it, when there is something. */
  hint?: string;
}

export interface PageFacts {
  crossOriginIsolated: boolean;
  isSecureContext: boolean;
  /** `location.origin`. */
  origin: string;
  hasWebAssembly: boolean;
  /** `new WebAssembly.Memory({ shared: true, ... })` works. */
  hasSharedMemory: boolean;
}

/** The browser and the page can run the node's threads; or why not. */
export function checkPage(page: PageFacts): BootFailure | null {
  if (!page.hasWebAssembly) {
    return {
      kind: "threads",
      title: "This browser can't run WebAssembly",
      detail:
        "The web node is a WebAssembly program. This browser has no WebAssembly, or has it turned off.",
      hint: "Open the page in a current Chrome, Edge, Firefox or Safari.",
    };
  }
  if (!page.isSecureContext) {
    return {
      kind: "isolation",
      title: "This page needs HTTPS",
      detail: `The page is open at ${page.origin}. Browsers give the node's threads shared memory only on HTTPS pages, or on localhost.`,
      hint: "Open it over HTTPS, or from localhost.",
    };
  }
  if (!page.crossOriginIsolated) {
    return {
      kind: "isolation",
      title: "The server didn't isolate this page",
      detail:
        "The node's threads share memory, which browsers allow only on a page sent with the headers Cross-Origin-Opener-Policy: same-origin and Cross-Origin-Embedder-Policy: require-corp. This server didn't send them.",
      hint: "Serve the page with both headers, as serve.json in the build sets them.",
    };
  }
  if (!page.hasSharedMemory) {
    return {
      kind: "threads",
      title: "This browser can't run WebAssembly threads",
      detail:
        "The page is isolated, but the browser refused shared WebAssembly memory. The node needs it for its threads.",
      hint: "Open the page in a current desktop browser.",
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
    crossOriginIsolated: globalThis.crossOriginIsolated === true,
    isSecureContext: globalThis.isSecureContext === true,
    origin: location.origin,
    hasWebAssembly: typeof WebAssembly === "object",
    hasSharedMemory,
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
  waitedMs: number,
): BootFailure {
  const blocked = relays
    .map(relay => blockedRelay(relay, pageOrigin))
    .filter((reason): reason is string => reason !== null);
  const seconds = Math.round(waitedMs / 1000);
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
        return seen ? `${host} (${seen.status.toLowerCase()})` : host;
      })
    : peers.map(p => `${p.address ?? "a peer"} (${p.status.toLowerCase()})`);
  return {
    kind: "peers",
    title: "No peer reachable",
    detail: `No connection in ${seconds} s. Tried ${tried.join(", ")}.`,
    hint: "The relay may be down, or this network may block WebRTC. The node keeps trying.",
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
