/**
 * React bindings: the client in context, polling that pauses with the tab,
 * the node's shared status, and one hook per endpoint. Each hook asks the
 * node through the client in context, so pointing the provider at another
 * node points every hook there.
 */

import * as React from "react";
import { useCallback, useEffect, useRef, useState } from "react";

import type { Block, Account } from "./chain";
import { createNodeClient, type NodeClient, type RawRoute } from "./client";
import { isUnreachable } from "./errors";
import type { PendingTransaction } from "./mempool";
import type { ProducerStats } from "./producer";
import type { CrashNotice } from "./report";
import type { RestoreSummary } from "./restore";
import {
  peerCounts,
  type BuildInfo,
  type NodeStatus,
  type Peer,
  type PeerCounts,
} from "./status";
import { STATUS_POLL_MS, syncView, type SyncView } from "./sync";
import type { IncludedWork, WorkerControl } from "./worker";

export interface Polled<T> {
  /** The last good answer, kept while later requests fail. */
  data: T | null;
  /** The latest request's failure; null once one succeeds again. */
  error: unknown;
  /** When `data` arrived, ms since the epoch. */
  updatedAt: number | null;
  /** No answer and no failure yet. */
  loading: boolean;
  refresh: () => void;
}

/**
 * Calls `fetcher` now and every `intervalMs` while the page is visible, and
 * again as soon as it becomes visible. A `key` change starts over; a null key
 * fetches nothing. While `paused`, it keeps the last answer and asks again
 * only on `refresh`.
 */
export function usePoll<T>(
  key: string | null,
  fetcher: (signal: AbortSignal) => Promise<T>,
  intervalMs: number,
  { paused = false }: { paused?: boolean } = {},
): Polled<T> {
  const [state, setState] = useState<{
    key: string | null;
    data: T | null;
    error: unknown;
    updatedAt: number | null;
  }>({ key, data: null, error: null, updatedAt: null });
  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;
  const [tick, setTick] = useState(0);
  const refresh = useCallback(() => setTick(t => t + 1), []);
  const askedTick = useRef(tick);

  useEffect(() => {
    if (key === null) return;
    // A refresh asks once even while paused; nothing else does.
    const asked = askedTick.current !== tick;
    askedTick.current = tick;
    if (paused && !asked) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;

    const run = async () => {
      clearTimeout(timer);
      controller?.abort();
      controller = new AbortController();
      const signal = controller.signal;
      try {
        const data = await fetcherRef.current(signal);
        if (cancelled || signal.aborted) return;
        setState({ key, data, error: null, updatedAt: Date.now() });
      } catch (error) {
        if (cancelled || signal.aborted) return;
        setState(s =>
          s.key === key
            ? { ...s, error }
            : { key, data: null, error, updatedAt: null },
        );
      }
      if (!cancelled && !paused && document.visibilityState === "visible") {
        timer = setTimeout(run, intervalMs);
      }
    };

    const onVisible = () => {
      if (document.visibilityState === "visible") void run();
      else clearTimeout(timer);
    };

    void run();
    if (!paused) document.addEventListener("visibilitychange", onVisible);
    return () => {
      cancelled = true;
      clearTimeout(timer);
      controller?.abort();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [key, intervalMs, tick, paused]);

  const current = state.key === key;
  return {
    data: current ? state.data : null,
    error: current ? state.error : null,
    updatedAt: current ? state.updatedAt : null,
    // An answer of null (no producer, no such block) is an answer: it ends
    // the loading as any other does.
    loading:
      key !== null &&
      (!current || (state.updatedAt === null && state.error === null)),
    refresh,
  };
}

const ClientContext = React.createContext<NodeClient | null>(null);

/** The client the hooks below use. Without a provider they use a client of
 * the page's own origin. */
export function NodeClientProvider({
  client,
  children,
}: {
  client: NodeClient;
  children: React.ReactNode;
}) {
  return (
    <ClientContext.Provider value={client}>{children}</ClientContext.Provider>
  );
}

let sameOrigin: NodeClient | null = null;

export function useNodeClient(): NodeClient {
  const client = React.useContext(ClientContext);
  if (client) return client;
  sameOrigin ??= createNodeClient();
  return sameOrigin;
}

interface NodeStatusValue {
  status: Polled<NodeStatus>;
  build: Polled<BuildInfo>;
  peers: PeerCounts | null;
}

const StatusContext = React.createContext<NodeStatusValue | null>(null);

/** Polls `/status` (every `STATUS_POLL_MS`) and `/build_env` once for every
 * component below it, so a header and the pages share one request. */
export function NodeStatusProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const client = useNodeClient();
  const status = usePoll(
    `status:${client.baseUrl}`,
    signal => client.status(signal),
    STATUS_POLL_MS,
  );
  // The build doesn't change while the node runs: ask again only if the
  // first answer failed.
  const build = usePoll(
    `build_env:${client.baseUrl}`,
    signal => client.buildInfo(signal),
    60_000,
  );
  const peers = React.useMemo(
    () => (status.data ? peerCounts(status.data.peers) : null),
    [status.data],
  );
  const value = React.useMemo(
    () => ({ status, build, peers }),
    [status, build, peers],
  );
  return (
    <StatusContext.Provider value={value}>{children}</StatusContext.Provider>
  );
}

/** The shared `/status` and `/build_env` of [`NodeStatusProvider`]. */
export function useNodeStatus(): NodeStatusValue {
  const value = React.useContext(StatusContext);
  if (!value) throw new Error("useNodeStatus outside NodeStatusProvider");
  return value;
}

/** The sync phase in words, from the shared status, at `now`. */
export function useSyncView(now: number): SyncView {
  const { status, peers } = useNodeStatus();
  const s = status.data;
  return syncView(
    {
      phase: s ? s.phase : null,
      bestTipHeight: s?.bestTip?.height ?? null,
      bestTipTimestampMs: s?.bestTip?.timestampMs ?? null,
      targetHeight: s?.target?.height ?? null,
      highestPeerTip: peers?.highestTip ?? null,
      sampledAt: status.updatedAt,
      unreachable: isUnreachable(status.error),
    },
    now,
  );
}

/** Polls `fetch(client)` under `key` (null: nothing). */
function useClientPoll<T>(
  key: string | null,
  fetch: (client: NodeClient, signal: AbortSignal) => Promise<T>,
  intervalMs: number,
  options?: { paused?: boolean },
): Polled<T> {
  const client = useNodeClient();
  return usePoll(
    key === null ? null : `${key}:${client.baseUrl}`,
    signal => fetch(client, signal),
    intervalMs,
    options,
  );
}

export const useSyncRestore = (
  intervalMs = 15_000,
): Polled<RestoreSummary | null> =>
  useClientPoll("stats/sync", (c, s) => c.syncRestore(s), intervalMs);

export const useWorkerControl = (intervalMs = 5_000): Polled<WorkerControl> =>
  useClientPoll("snarker/control", (c, s) => c.workerControl(s), intervalMs);

export const useIncludedWork = (intervalMs = 15_000): Polled<IncludedWork> =>
  useClientPoll("snarker/included", (c, s) => c.includedWork(s), intervalMs);

export const useProducerStats = (
  intervalMs = 10_000,
): Polled<ProducerStats | null> =>
  useClientPoll(
    "stats/block_producer",
    (c, s) => c.producerStats(s),
    intervalMs,
  );

export const useTransactionPool = (
  intervalMs = 5_000,
): Polled<PendingTransaction[]> =>
  useClientPoll("transaction-pool", (c, s) => c.transactionPool(s), intervalMs);

export const usePeers = (intervalMs = 5_000): Polled<Peer[]> =>
  useClientPoll("state/peers", (c, s) => c.peers(s), intervalMs);

export const useBestChain = (
  length?: number,
  intervalMs = 10_000,
): Polled<Block[]> =>
  useClientPoll(
    `bestChain:${length ?? ""}`,
    (c, s) => c.bestChain(length, s),
    intervalMs,
  );

/** A block doesn't change: asked again only after a failure (every 60 s). */
export const useBlock = (id: string | null): Polled<Block | null> =>
  useClientPoll(
    id === null ? null : `block:${id}`,
    (c, s) => c.block(id!, s),
    60_000,
  );

export const useAccount = (
  publicKey: string | null,
  intervalMs = 30_000,
): Polled<Account | null> =>
  useClientPoll(
    publicKey === null ? null : `account:${publicKey}`,
    (c, s) => c.account(publicKey!, s),
    intervalMs,
  );

/** A read-only route's JSON, with polling that can pause. */
export const useRaw = (
  route: RawRoute,
  {
    paused = false,
    intervalMs = STATUS_POLL_MS,
  }: { paused?: boolean; intervalMs?: number } = {},
): Polled<unknown> =>
  useClientPoll(`raw:${route}`, (c, s) => c.raw(route, s), intervalMs, {
    paused,
  });

/** Whether the node crashed lately (`POST /report/crash`), for a notice;
 * a node too old to say, or one that refuses, gives an error and no data. */
export const useCrashNotice = (
  intervalMs = 60_000,
): Polled<CrashNotice | null> =>
  useClientPoll("report/crash", (c, s) => c.crashNotice(s), intervalMs);
