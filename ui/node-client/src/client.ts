/**
 * A typed client for a Mina Rust node's HTTP API and GraphQL. Requests go
 * through a transport: HTTP to `baseUrl` by default (empty: the page's own
 * origin, as the dashboard the node serves uses it; or any node, such as
 * `http://192.0.2.44:3600`), or calls into a web node running in the page
 * (`createWebNodeTransport` in `./webnode`).
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
import {
  parseBestChainHeaders,
  parseBlockDetail,
  parseLedgerAccounts,
  parseSyncProgress,
  type BestChainHeaders,
  type BlockDetail,
  type LedgerAccount,
  type SyncProgress,
} from "./chainview";
import { NodeError } from "./errors";
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
import {
  parseLedgerSync,
  parseSyncBlocks,
  type LedgerSync,
  type SyncBlocks,
} from "./progress";
import { parseSyncRestore, type RestoreSummary } from "./restore";
import { httpTransport, type NodeTransport } from "./transport";
import {
  parseBuildInfo,
  parsePeers,
  parseStatus,
  type BuildInfo,
  type NodeStatus,
  type Peer,
} from "./status";
import {
  parseVerification,
  VERIFICATION_MAX_PAGE,
  VERIFICATION_PAGE,
  type Verification,
} from "./verification";
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
  /** How to reach the node, instead of HTTP to `baseUrl`: a web node in
   * this page, for one. The options above are then unused. */
  transport?: NodeTransport;
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
  /** The ledgers a bootstrap fetches, from `GET /state/message-progress`. */
  ledgerSync(signal?: AbortSignal): Promise<LedgerSync>;
  /** The blocks of the newest sync, from `GET /stats/sync?limit=1`. */
  syncBlocks(signal?: AbortSignal): Promise<SyncBlocks | null>;
  /** `GET /stats/verification`: the checks of the blocks this node
   * received, newest first, and its counters since start. */
  verification(
    page?: number | { limit?: number; offset?: number },
    signal?: AbortSignal,
  ): Promise<Verification>;
  /** `GET /stats/verification?hashes=`: these blocks' receipts, at most
   * 128; a block without one isn't in the answer. */
  verificationOf(hashes: string[], signal?: AbortSignal): Promise<Verification>;
  /** `GET /best-chain/headers`: the best chain's headers, newest first. */
  bestChainHeaders(
    query?: { limit?: number; maxHeight?: number },
    signal?: AbortSignal,
  ): Promise<BestChainHeaders>;
  /** `GET /best-chain/block/{id}`, by height or state hash; null if the
   * node's frontier hasn't got it. */
  chainBlock(id: string, signal?: AbortSignal): Promise<BlockDetail | null>;
  /** `GET /stats/sync/progress`: the sync, ledger by ledger and block by
   * block. */
  syncProgress(signal?: AbortSignal): Promise<SyncProgress>;
  /** `GET /accounts/{publicKey}` (a web node): the key's accounts at the
   * best tip; empty if the ledger has none, null without a ledger. */
  ledgerAccounts(
    publicKey: string,
    signal?: AbortSignal,
  ): Promise<LedgerAccount[] | null>;
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

export function createNodeClient(options: NodeClientOptions = {}): NodeClient {
  const transport = options.transport ?? httpTransport(options);

  const client: NodeClient = {
    baseUrl: transport.baseUrl,
    url: path => transport.url(path),
    async getJson(path, parse, signal) {
      return parse(await transport.request({ method: "GET", path, signal }));
    },
    async postJson(path, payload, parse, headers = {}, signal) {
      return parse(
        await transport.request({
          method: "POST",
          path,
          payload,
          headers,
          signal,
        }),
      );
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
    ledgerSync: signal =>
      client.getJson("/state/message-progress", parseLedgerSync, signal),
    syncBlocks: signal =>
      client.getJson("/stats/sync?limit=1", parseSyncBlocks, signal),
    verification(page = VERIFICATION_PAGE, signal) {
      const { limit = VERIFICATION_PAGE, offset = 0 } =
        typeof page === "number" ? { limit: page } : page;
      const count = (n: number) => Math.max(0, Math.floor(n));
      return client.getJson(
        `/stats/verification?limit=${count(limit)}${offset ? `&offset=${count(offset)}` : ""}`,
        parseVerification,
        signal,
      );
    },
    verificationOf: (hashes, signal) =>
      client.getJson(
        `/stats/verification?hashes=${hashes
          .slice(0, VERIFICATION_MAX_PAGE)
          .map(encodeURIComponent)
          .join(",")}`,
        parseVerification,
        signal,
      ),
    bestChainHeaders({ limit, maxHeight } = {}, signal) {
      const params = new URLSearchParams();
      if (limit !== undefined)
        params.set("limit", String(Math.max(0, Math.floor(limit))));
      if (maxHeight !== undefined)
        params.set("max_height", String(Math.max(0, Math.floor(maxHeight))));
      const query = params.toString();
      return client.getJson(
        `/best-chain/headers${query ? `?${query}` : ""}`,
        parseBestChainHeaders,
        signal,
      );
    },
    chainBlock: (id, signal) =>
      client.getJson(
        `/best-chain/block/${encodeURIComponent(id.trim())}`,
        parseBlockDetail,
        signal,
      ),
    syncProgress: signal =>
      client.getJson("/stats/sync/progress", parseSyncProgress, signal),
    ledgerAccounts: (publicKey, signal) =>
      client.getJson(
        `/accounts/${encodeURIComponent(publicKey.trim())}`,
        parseLedgerAccounts,
        signal,
      ),
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
