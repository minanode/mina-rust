/**
 * How a client reaches a node. Over HTTP to its API, which is every node the
 * dashboard reads, or by calls into a web node running in the same page
 * (`./webnode`). The client's methods and parsers are the same for both: a
 * transport only takes a route and gives back the JSON the node answered.
 */

import { errorKind, errorMessage, NodeError } from "./errors";

export interface TransportRequest {
  method: "GET" | "POST";
  /** The route, with its query: `/stats/sync?limit=1`. */
  path: string;
  /** A POST's JSON body. */
  payload?: unknown;
  headers?: Record<string, string>;
  signal?: AbortSignal;
}

export interface NodeTransport {
  /** Names the node: a base URL for HTTP, `webnode:` for one in the page.
   * The hooks key their answers by it. */
  readonly baseUrl: string;
  /** Where `path` is on the node, to show or link to. */
  url(path: string): string;
  /** The node's answer as JSON, null for an empty one. Throws a NodeError. */
  request(request: TransportRequest): Promise<unknown>;
}

export interface HttpTransportOptions {
  /** `""` (default) for the page's own origin, or an origin such as
   * `http://127.0.0.1:3000`, with or without a path. */
  baseUrl?: string;
  /** The fetch to use (tests, other runtimes). Default: the global one. */
  fetch?: typeof fetch;
  /** Give up on an answer after this long. Default 15 s. */
  timeoutMs?: number;
}

const DEFAULT_TIMEOUT_MS = 15_000;

/** The node's HTTP API at `baseUrl`. */
export function httpTransport(
  options: HttpTransportOptions = {},
): NodeTransport {
  const baseUrl = (options.baseUrl ?? "").replace(/\/+$/, "");
  const doFetch =
    options.fetch ?? ((...args: Parameters<typeof fetch>) => fetch(...args));
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const url = (path: string) =>
    `${baseUrl}${path.startsWith("/") ? path : `/${path}`}`;

  async function send(
    path: string,
    init: RequestInit & { signal?: AbortSignal },
  ) {
    const signals = [AbortSignal.timeout(timeoutMs)];
    if (init.signal) signals.push(init.signal);
    // No answer, or the answer broke off: the node is offline, unless the
    // caller gave up.
    const offline = (error: unknown, what: string) => {
      if (init.signal?.aborted) return error;
      return new NodeError(
        "offline",
        error instanceof DOMException && error.name === "TimeoutError"
          ? `The node didn't answer in ${Math.round(timeoutMs / 1000)} s.`
          : what,
      );
    };
    let response: Response;
    try {
      response = await doFetch(url(path), {
        ...init,
        signal: AbortSignal.any(signals),
      });
    } catch (error) {
      throw offline(error, "The node isn't answering.");
    }
    let body: string;
    try {
      body = await response.text();
    } catch (error) {
      throw offline(error, "The node stopped answering mid-reply.");
    }
    if (!response.ok) {
      const fallback =
        response.status === 404
          ? `This node doesn't serve ${path.split("?")[0]}.`
          : `The node answered HTTP ${response.status}.`;
      throw new NodeError(
        errorKind(response.status),
        errorMessage(body) ?? fallback,
        response.status,
      );
    }
    return body;
  }

  return {
    baseUrl,
    url,
    async request({ method, path, payload, headers = {}, signal }) {
      const body = await send(
        path,
        method === "GET"
          ? { headers: { Accept: "application/json", ...headers }, signal }
          : {
              method: "POST",
              headers: {
                Accept: "application/json",
                "Content-Type": "application/json",
                ...headers,
              },
              body: JSON.stringify(payload),
              signal,
            },
      );
      try {
        return body.trim() ? JSON.parse(body) : null;
      } catch {
        throw new NodeError("invalid", "The node's answer isn't JSON.");
      }
    },
  };
}
