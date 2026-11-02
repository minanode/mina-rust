/**
 * A transport to a web node running in this page: the `RpcSender` that the
 * WASM's `run()` resolves to (crates/node/common/src/service/rpc/sender.rs).
 * It answers the routes a web node has, with the JSON the native node's HTTP
 * API would send, so the client's parsers and the pages' hooks work as they
 * do over HTTP. The SNARK worker's state and included work come from
 * `roles()`, which a web node of MINAWEB-P2-WRITE or later has; a worker is
 * started and stopped by starting the node again (`run`), so a change to
 * it is refused here. Routes it hasn't got (GraphQL, the bug report) fail
 * as `absent`, saying so.
 */

import { NodeError } from "./errors";
import type { NodeTransport, TransportRequest } from "./transport";

/** The parts of the WASM's `RpcSender` this transport calls. */
export interface WebNodeRpc {
  status(): Promise<unknown>;
  state(): { peers(): Promise<unknown>; message_progress(): Promise<unknown> };
  stats(): {
    sync(limit?: number): Promise<unknown>;
    block_producer(): Promise<unknown>;
    verification?(
      limit?: number,
      offset?: number,
      hashes?: string[],
    ): Promise<unknown>;
    sync_progress?(): Promise<unknown>;
  };
  transaction_pool(): { get(): Promise<unknown> };
  transition_frontier?(): {
    best_chain(): {
      headers?(limit?: number, maxHeight?: number): Promise<unknown>;
      block?(id: string): Promise<unknown>;
    };
  };
  ledger?(): {
    latest(): { accounts(): { get?(publicKey: string): Promise<unknown> } };
  };
  /** What a page's role screens read; absent from older web nodes. */
  roles?(): {
    snarker_control(): Promise<unknown>;
    snarker_included(): Promise<unknown>;
  };
}

export interface WebNodeTransportOptions {
  /** The WASM's `build_env()`, for `/build_env`. */
  buildEnv?: () => unknown;
  /** Give up on an answer after this long. Default 15 s. */
  timeoutMs?: number;
}

export const WEB_NODE_BASE = "webnode:";

const DEFAULT_TIMEOUT_MS = 15_000;

/** What a web node can't answer, by route, and why. */
const ABSENT: [RegExp, string][] = [
  [/^\/graphql$/, "The web node has no GraphQL API."],
  [
    /^\/snarker\//,
    "This web node can't report a SNARK worker: it is older than the page.",
  ],
  [/^\/report(\/|$)/, "The web node has no bug report."],
];

function count(params: URLSearchParams, name: string): number | undefined {
  const value = params.get(name);
  if (value === null || !/^\d+$/.test(value)) return undefined;
  return Number(value);
}

export function createWebNodeTransport(
  rpc: WebNodeRpc,
  options: WebNodeTransportOptions = {},
): NodeTransport {
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  // Each of these is a WASM object; one of each serves every request.
  const state = rpc.state();
  const stats = rpc.stats();
  const pool = rpc.transaction_pool();
  const bestChain = rpc.transition_frontier?.().best_chain();
  const accounts = rpc.ledger?.().latest().accounts();
  const tooOld = (what: string) =>
    new NodeError("absent", `This web node is too old to ${what}.`);

  /** Routes with a path parameter: a prefix, and the handler of the rest. */
  const prefixed: [string, (rest: string) => Promise<unknown>][] = [
    [
      "/best-chain/block/",
      rest => {
        if (typeof bestChain?.block !== "function") {
          throw tooOld("show a block");
        }
        return bestChain.block(decodeURIComponent(rest));
      },
    ],
    [
      "/accounts/",
      rest => {
        if (typeof accounts?.get !== "function") {
          throw tooOld("look up accounts");
        }
        return accounts.get(decodeURIComponent(rest));
      },
    ],
  ];
  const roles = typeof rpc.roles === "function" ? rpc.roles() : null;

  const routes: Record<string, (params: URLSearchParams) => Promise<unknown>> =
    {
      "/status": async () => {
        const status = await rpc.status();
        // The node thread answers no status only when it has stopped.
        if (status === null || status === undefined) {
          throw new NodeError("failed", "The web node gave no status.");
        }
        return status;
      },
      "/build_env": async () => {
        if (!options.buildEnv) {
          throw new NodeError("absent", "This web node doesn't say its build.");
        }
        return options.buildEnv();
      },
      "/stats/sync": params => stats.sync(count(params, "limit")),
      "/stats/block_producer": () => stats.block_producer(),
      "/stats/verification": params => {
        if (typeof stats.verification !== "function") {
          throw new NodeError(
            "absent",
            "This web node is too old to report its block checks.",
          );
        }
        const hashes = (params.get("hashes") ?? "")
          .split(",")
          .map(h => h.trim())
          .filter(h => h.length > 0);
        return hashes.length > 0
          ? stats.verification(
              count(params, "limit"),
              count(params, "offset"),
              hashes,
            )
          : stats.verification(count(params, "limit"), count(params, "offset"));
      },
      "/stats/sync/progress": () => {
        if (typeof stats.sync_progress !== "function") {
          throw tooOld("report its sync progress");
        }
        return stats.sync_progress();
      },
      "/best-chain/headers": params => {
        if (typeof bestChain?.headers !== "function") {
          throw tooOld("list the best chain");
        }
        return bestChain.headers(
          count(params, "limit"),
          count(params, "max_height"),
        );
      },
      "/state/peers": () => state.peers(),
      "/state/message-progress": () => state.message_progress(),
      "/transaction-pool": () => pool.get(),
      ...(roles
        ? {
            "/snarker/control": () => roles.snarker_control(),
            "/snarker/included": () => roles.snarker_included(),
          }
        : {}),
    };

  async function answer(route: string, params: URLSearchParams) {
    const match = prefixed.find(
      ([prefix]) => route.startsWith(prefix) && route.length > prefix.length,
    );
    const handler: ((params: URLSearchParams) => Promise<unknown>) | undefined =
      routes[route] ??
      (match ? () => match[1](route.slice(match[0].length)) : undefined);
    if (!handler) {
      const absent = ABSENT.find(([pattern]) => pattern.test(route));
      throw new NodeError(
        "absent",
        absent?.[1] ?? `The web node doesn't serve ${route}.`,
        404,
      );
    }
    try {
      const value = await handler(params);
      return value === undefined ? null : value;
    } catch (error) {
      if (error instanceof NodeError) throw error;
      // A JsError or a panic message from the WASM.
      throw new NodeError(
        "failed",
        `The web node failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  return {
    baseUrl: WEB_NODE_BASE,
    url: path => `${WEB_NODE_BASE}${path.startsWith("/") ? path : `/${path}`}`,
    request({ method, path, signal }: TransportRequest) {
      const [route, query = ""] = path.split("?", 2);
      if (method !== "GET") {
        return Promise.reject(
          new NodeError(
            "absent",
            route.startsWith("/snarker/")
              ? "A web node's SNARK worker starts and stops with the node: use the page's SNARK worker screen."
              : (ABSENT.find(([pattern]) => pattern.test(route))?.[1] ??
                  "The web node takes no requests that change it."),
            405,
          ),
        );
      }
      return new Promise((resolve, reject) => {
        // The node thread may be busy for a while, or stopped: give up as
        // the HTTP transport does, and on the caller's abort.
        const timer = setTimeout(
          () =>
            reject(
              new NodeError(
                "offline",
                `The web node didn't answer in ${Math.round(timeoutMs / 1000)} s.`,
              ),
            ),
          timeoutMs,
        );
        const onAbort = () => {
          clearTimeout(timer);
          reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
        };
        if (signal?.aborted) return onAbort();
        signal?.addEventListener("abort", onAbort, { once: true });
        answer(route, new URLSearchParams(query)).then(
          value => {
            clearTimeout(timer);
            signal?.removeEventListener("abort", onAbort);
            resolve(value);
          },
          error => {
            clearTimeout(timer);
            signal?.removeEventListener("abort", onAbort);
            reject(error);
          },
        );
      });
    },
  };
}
