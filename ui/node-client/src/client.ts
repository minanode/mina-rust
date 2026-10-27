/**
 * A typed client for a Mina Rust node's HTTP API and GraphQL. Every request
 * goes to `baseUrl` (empty: the page's own origin, as the dashboard the node
 * serves uses it; or any node, such as `http://192.0.2.44:3600`).
 */

import {
  ACCOUNT,
  blockQuery,
  parseAccount,
  parseBestChain,
  parseBlockAnswer,
  RECENT_BLOCKS,
  RECENT_BLOCKS_LENGTH,
  type Account,
  type Block,
} from "./chain";
import { errorKind, errorMessage, NodeError } from "./errors";
import { obj, str } from "./json";
import { parseTransactionPool, type PendingTransaction } from "./mempool";
import { parseProducerStats, type ProducerStats } from "./producer";
import {
  parseCrashNotice,
  parseReport,
  REPORT_HEADER,
  type CrashNotice,
  type Report,
  type ReportOptions,
} from "./report";
import { parseSyncRestore, type RestoreSummary } from "./restore";
import {
  parseBuildInfo,
  parsePeers,
  parseStatus,
  type BuildInfo,
  type NodeStatus,
  type Peer,
} from "./status";
import {
  parseIncluded,
  parseWorkerControl,
  WORKER_CONTROL_HEADER,
  type IncludedWork,
  type WorkerChange,
  type WorkerControl,
} from "./worker";

export interface NodeClientOptions {
  /** Where the node is: `""` (default) for the page's own origin, or an
   * origin such as `http://127.0.0.1:3000`, with or without a path. */
  baseUrl?: string;
  /** The fetch to use (tests, other runtimes). Default: the global one. */
  fetch?: typeof fetch;
  /** Give up on an answer after this long. Default 15 s. */
  timeoutMs?: number;
}

/** Read-only GET routes a UI may show raw: no `/state`, nothing that
 * changes the node. */
export const RAW_ROUTES = ["/status", "/build_env"] as const;
export type RawRoute = (typeof RAW_ROUTES)[number];

export interface NodeClient {
  readonly baseUrl: string;
  /** The absolute (or origin-relative) URL of `path` on the node. */
  url(path: string): string;
  getJson<T>(
    path: string,
    parse: (raw: unknown) => T,
    signal?: AbortSignal,
  ): Promise<T>;
  postJson<T>(
    path: string,
    payload: unknown,
    parse: (raw: unknown) => T,
    headers?: Record<string, string>,
    signal?: AbortSignal,
  ): Promise<T>;
  /** The node's GraphQL: `data`, or its first error as a NodeError. */
  graphql<T>(
    query: string,
    variables: Record<string, unknown>,
    parse: (data: Record<string, unknown>) => T,
    signal?: AbortSignal,
  ): Promise<T>;

  /** `GET /status`. */
  status(signal?: AbortSignal): Promise<NodeStatus>;
  /** `GET /build_env`. */
  buildInfo(signal?: AbortSignal): Promise<BuildInfo>;
  /** The restore of this run, from `GET /stats/sync?limit=1`; null after a
   * bootstrap. */
  syncRestore(signal?: AbortSignal): Promise<RestoreSummary | null>;
  /** `GET /stats/block_producer`; null without a producer. */
  producerStats(signal?: AbortSignal): Promise<ProducerStats | null>;
  /** `GET /snarker/control`. */
  workerControl(signal?: AbortSignal): Promise<WorkerControl>;
  /** `POST /snarker/control`: start, re-price or stop the SNARK worker. */
  changeWorker(
    change: WorkerChange,
    signal?: AbortSignal,
  ): Promise<WorkerControl>;
  /** `GET /snarker/included`. */
  includedWork(signal?: AbortSignal): Promise<IncludedWork>;
  /** `POST /report`: the node's redacted bug report and its issue link.
   * The operator's request only (the report header; this machine, the
   * node's own dashboard or a listed origin). */
  report(options?: ReportOptions, signal?: AbortSignal): Promise<Report>;
  /** `POST /report/crash`: whether the node crashed lately. */
  crashNotice(signal?: AbortSignal): Promise<CrashNotice | null>;
  /** `GET /transaction-pool`, highest fee first. */
  transactionPool(signal?: AbortSignal): Promise<PendingTransaction[]>;
  /** `GET /state/peers`. */
  peers(signal?: AbortSignal): Promise<Peer[]>;
  /** A read-only route's JSON as it comes. */
  raw(route: RawRoute, signal?: AbortSignal): Promise<unknown>;
  /** GraphQL `bestChain`, newest first. */
  bestChain(length?: number, signal?: AbortSignal): Promise<Block[]>;
  /** GraphQL `block` by height or state hash; throws `refused` for an id
   * that is neither. */
  block(id: string, signal?: AbortSignal): Promise<Block | null>;
  /** GraphQL `account`; null when the ledger has none for the key. */
  account(publicKey: string, signal?: AbortSignal): Promise<Account | null>;
}

const DEFAULT_TIMEOUT_MS = 15_000;

export function createNodeClient(options: NodeClientOptions = {}): NodeClient {
  const baseUrl = (options.baseUrl ?? "").replace(/\/+$/, "");
  const doFetch =
    options.fetch ?? ((...args: Parameters<typeof fetch>) => fetch(...args));
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const url = (path: string) =>
    `${baseUrl}${path.startsWith("/") ? path : `/${path}`}`;

  async function request(
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

  function parseBody<T>(body: string, parse: (raw: unknown) => T): T {
    let raw: unknown;
    try {
      raw = body.trim() ? JSON.parse(body) : null;
    } catch {
      throw new NodeError("invalid", "The node's answer isn't JSON.");
    }
    return parse(raw);
  }

  const client: NodeClient = {
    baseUrl,
    url,
    async getJson(path, parse, signal) {
      const body = await request(path, {
        headers: { Accept: "application/json" },
        signal,
      });
      return parseBody(body, parse);
    },
    async postJson(path, payload, parse, headers = {}, signal) {
      const body = await request(path, {
        method: "POST",
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
          ...headers,
        },
        body: JSON.stringify(payload),
        signal,
      });
      return parseBody(body, parse);
    },
    graphql(query, variables, parse, signal) {
      return client.postJson(
        "/graphql",
        { query, variables },
        raw => {
          const response = obj(raw);
          const errors = Array.isArray(response?.errors) ? response.errors : [];
          const data = obj(response?.data);
          if (errors.length > 0 && !data) {
            const message = str(obj(errors[0])?.message) ?? "GraphQL error";
            throw new NodeError("refused", message);
          }
          if (!data)
            throw new NodeError("invalid", "GraphQL answered no data.");
          if (errors.length > 0) {
            // A field failed (a block or account not found): say why.
            const message = str(obj(errors[0])?.message);
            if (message) throw new NodeError("refused", message);
          }
          return parse(data);
        },
        {},
        signal,
      );
    },
    status: signal => client.getJson("/status", parseStatus, signal),
    buildInfo: signal => client.getJson("/build_env", parseBuildInfo, signal),
    syncRestore: signal =>
      client.getJson("/stats/sync?limit=1", parseSyncRestore, signal),
    producerStats: signal =>
      client.getJson("/stats/block_producer", parseProducerStats, signal),
    workerControl: signal =>
      client.getJson("/snarker/control", parseWorkerControl, signal),
    changeWorker: (change, signal) =>
      client.postJson(
        "/snarker/control",
        change,
        parseWorkerControl,
        { [WORKER_CONTROL_HEADER]: "1" },
        signal,
      ),
    includedWork: signal =>
      client.getJson("/snarker/included", parseIncluded, signal),
    report: (options = {}, signal) =>
      client.postJson(
        "/report",
        {
          include_network_ids: options.includeNetworkIds ?? false,
          ...(options.description?.trim()
            ? { description: options.description.trim() }
            : {}),
        },
        parseReport,
        { [REPORT_HEADER]: "1" },
        signal,
      ),
    crashNotice: signal =>
      client.postJson(
        "/report/crash",
        {},
        parseCrashNotice,
        { [REPORT_HEADER]: "1" },
        signal,
      ),
    transactionPool: signal =>
      client.getJson("/transaction-pool", parseTransactionPool, signal),
    peers: signal => client.getJson("/state/peers", parsePeers, signal),
    raw: (route, signal) => client.getJson(route, value => value, signal),
    bestChain: (length = RECENT_BLOCKS_LENGTH, signal) =>
      client.graphql(
        RECENT_BLOCKS,
        { maxLength: length },
        parseBestChain,
        signal,
      ),
    block(id, signal) {
      const query = blockQuery(id);
      if (!query) {
        return Promise.reject(
          new NodeError(
            "refused",
            "That isn't a block height or a state hash.",
          ),
        );
      }
      return client.graphql(
        query.query,
        query.variables,
        parseBlockAnswer,
        signal,
      );
    },
    account: (publicKey, signal) =>
      client.graphql(ACCOUNT, { publicKey }, parseAccount, signal),
  };
  return client;
}
