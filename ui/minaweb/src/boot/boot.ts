/**
 * Starting the web node, in steps the boot screen shows: check the browser,
 * download the WASM, start the node, connect to a peer, sync the ledgers,
 * sync the blocks. One boot per page: the store below holds its state, and
 * React reads it with useSyncExternalStore.
 */

import {
  createNodeClient,
  createWebNodeTransport,
  formatCount,
  formatHeight,
  ledgerTotal,
  type LedgerSync,
  type NodeClient,
  type SyncProgress,
  type WebNodeRpc,
} from "@minanode/node-client";

import { reloadAndStart } from "./autostart";
import type { BuildConfig } from "./build-config";
import {
  BootError,
  compressedSize,
  downloadWasm,
  type DownloadProgress,
} from "./download";
import {
  checkPage,
  formatBytes,
  noPeerFailure,
  pageFacts,
  type BootFailure,
} from "./failures";
import {
  admitIceLists,
  joinIceServers,
  loadEachIceServers,
  renewEachIceServers,
} from "./ice";
import {
  blockStrip,
  ledgerStepSentence,
  rememberLedgers,
  stripSentence,
  type LedgerMemory,
} from "./progress";
import { takeHandoff } from "../roles/handoff";
import {
  decryptKey,
  HANDOFF_KEY,
  keyStore,
  readStored,
  removeStored,
  type HeldKey,
  type KeyFileGlue,
  type KeyGlue,
} from "../roles/keys";
import { readSharing } from "./sharing";
import { pendingSteps, type Step, type StepId } from "./steps";
import {
  readRole,
  rolesAllowed,
  runOptions,
  runRoleArgs,
  VIEWER,
  writeRole,
  type Role,
  type RunRoleArgs,
} from "../roles/role";

/** The WASM glue that wasm-bindgen writes (mina_node_web.js). */
export interface WebNodeGlue extends Partial<KeyGlue>, Partial<KeyFileGlue> {
  default(init: {
    module_or_path: BufferSource;
    memory: WebAssembly.Memory;
  }): Promise<unknown>;
  run(
    blockProducer: string | null,
    seedUrls: string[],
    seeds: string[],
    genesisUrl: string | null,
    network: string,
    coinbaseReceiver: string | null,
    snarker: string | null,
    snarkerFee: string | null,
    options: Record<string, unknown>,
  ): Promise<WebNodeRpc>;
  build_env(): unknown;
  memory_stats?(): unknown;
  update_ice_servers?(servers: import("./ice").IceServer[]): void;
  /** Each open WebRTC connection's peer, route, round trip and bytes. */
  webrtc_peer_stats?(): Promise<unknown>;
}

export type { Step, StepId, StepState } from "./steps";

export interface BootSnapshot {
  steps: Step[];
  failure: BootFailure | null;
  download: DownloadProgress | null;
  /** The node runs and answers. */
  client: NodeClient | null;
  glue: WebNodeGlue | null;
  /** The WASM's RpcSender, for the role screens' calls (roles/node.ts). */
  rpc: WebNodeRpc | null;
  /** It has been Synced once. */
  synced: boolean;
  /** The frontier store is on for this run. */
  frontierStore: boolean;
  /** This run started by deleting the store. */
  frontierStoreReset: boolean;
  /** This run shares with other browsers (boot/sharing.ts). */
  sharing: boolean;
  /** The sync's progress while it runs, from `/stats/sync/progress`. */
  progress: SyncProgress | null;
  /** Each ledger's last counts while it synced. */
  ledgerMemory: LedgerMemory;
  /** Where the start step is, an index of START_PARTS. */
  startPart: number;
  /** The connect step's live counts. */
  connect: ConnectCounts | null;
  /** What this tab's node does (roles/role.ts). */
  role: Role;
  /** A producer's restart waits for its key's passphrase. */
  unlock: { publicKey: string; error: string | null; busy: boolean } | null;
}

/** The start step's parts, in order. */
export const START_PARTS = [
  "Compile the WebAssembly",
  "Fetch the genesis config and peers",
  "Build the verifier keys and the node",
] as const;

export interface ConnectCounts {
  /** Relays the build dials. */
  relays: number;
  /** Peers the node is handshaking with. */
  connecting: number;
  connected: number;
  /** When the node started dialling, ms since the epoch. */
  since: number;
}

/** How long without a connected peer before the screen says why. */
export const PEER_WAIT_MS = 45_000;
const POLL_MS = 1_000;
/** The poll while the node dials: the first peer shows within this. */
const CONNECT_POLL_MS = 200;
/** How long the dial is polled that often: a peer connects within a few
 * seconds, or the dial is failing. */
export const CONNECT_FAST_MS = 10_000;
/** The poll of a node still without a peer a minute after the screen said
 * why (PEER_WAIT_MS). */
const NO_PEER_POLL_MS = 5_000;

/**
 * How long the boot waits before it asks the node again, `dialledMs` after
 * it started dialling: every 200 ms in the first 10 s, so that the first
 * peer shows at once, then once a second, as once a peer is connected, and
 * every 5 s from a minute after the screen said why there is no peer.
 */
export function pollDelay(connected: boolean, dialledMs: number): number {
  if (connected) return POLL_MS;
  if (dialledMs < CONNECT_FAST_MS) return CONNECT_POLL_MS;
  if (dialledMs < PEER_WAIT_MS + 60_000) return POLL_MS;
  return NO_PEER_POLL_MS;
}

/** localStorage `webnodeFrontierStore` (`on` or `off`) overrides the build,
 * as on the Angular page. */
export function frontierStoreChoice(
  built: boolean,
  stored: string | null,
): boolean {
  if (stored === "on") return true;
  if (stored === "off") return false;
  return built;
}

/**
 * The ledger and block steps when the sync is at `stage`. Exactly one of the
 * two is current. A sync that restarts (a new target moves it from its
 * blocks back to its ledgers) reopens the ledger step, whose time then runs
 * on from when it first started, and puts the block step back to pending,
 * as it will start again.
 */
export function syncSteps(
  steps: Step[],
  stage: "ledgers" | "blocks",
  now: number,
): Step[] {
  return steps.map(s => {
    if (s.id === "ledgers") {
      if (stage === "ledgers") {
        return s.state === "current"
          ? s
          : {
              ...s,
              state: "current",
              startedAt: s.startedAt ?? now,
              endedAt: null,
            };
      }
      return s.state === "done"
        ? s
        : { ...s, state: "done", startedAt: s.startedAt ?? now, endedAt: now };
    }
    if (s.id === "blocks") {
      if (stage === "blocks") {
        return s.state === "current"
          ? s
          : {
              ...s,
              state: "current",
              startedAt: s.startedAt ?? now,
              endedAt: null,
            };
      }
      return s.state === "pending"
        ? s
        : {
            ...s,
            state: "pending",
            detail: null,
            startedAt: null,
            endedAt: null,
          };
    }
    return s;
  });
}

/** sessionStorage: delete the frontier store on the next start. */
export const RESET_KEY = "minawebFrontierStoreReset";

/** Deletes this browser's frontier store and restarts the node: the page
 * reloads, and the next run starts with the store deleted. */
export function resetFrontierStore() {
  try {
    sessionStorage.setItem(RESET_KEY, "1");
  } catch {
    // Without sessionStorage there is nothing to carry the request.
    return false;
  }
  reloadAndStart();
  return true;
}

/** Whether this start was asked to delete the store; asks once. */
function takeResetRequest(): boolean {
  try {
    const asked = sessionStorage.getItem(RESET_KEY) === "1";
    sessionStorage.removeItem(RESET_KEY);
    return asked;
  } catch {
    return false;
  }
}

/** What the ledger step says while it runs. */
export function ledgerDetail(sync: LedgerSync): string {
  const parts: [string, LedgerSync["root"]][] = [
    ["Staking epoch ledger", sync.stakingEpoch],
    ["Next epoch ledger", sync.nextEpoch],
    ["Root ledger", sync.root],
  ];
  const current = parts.find(([, p]) => p !== null && p.done < p.total);
  if (current && current[1]) {
    return `${current[0]}: ${formatCount(current[1].done)} of about ${formatCount(current[1].total)} accounts.`;
  }
  if (sync.rootStaged && sync.rootStaged.done < sync.rootStaged.total) {
    return `Root staged ledger: ${formatCount(sync.rootStaged.done)} of ${formatCount(sync.rootStaged.total)} parts.`;
  }
  const total = ledgerTotal(sync);
  return total
    ? `${formatCount(total.done)} accounts fetched.`
    : "Asking peers for the ledgers.";
}

function seconds(ms: number): string {
  return ms < 10_000
    ? `${(ms / 1000).toFixed(1)} s`
    : `${Math.round(ms / 1000)} s`;
}

export class Boot {
  private snapshot: BootSnapshot;
  private listeners = new Set<() => void>();
  private started = false;
  private keyWaiter: ((key: HeldKey | null) => void) | null = null;
  private glueRef: WebNodeGlue | null = null;
  /** The key a role change carried, taken at the page's load. */
  private carried: Promise<HeldKey | null> | null = null;

  constructor(private readonly config: BuildConfig) {
    this.snapshot = {
      steps: pendingSteps(),
      failure: null,
      download: null,
      client: null,
      glue: null,
      rpc: null,
      synced: false,
      frontierStore: config.frontierStore,
      frontierStoreReset: false,
      sharing: readSharing(),
      progress: null,
      ledgerMemory: {},
      startPart: 0,
      connect: null,
      role: rolesAllowed(config.network) ? readRole(config.network) : VIEWER,
      unlock: null,
    };
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = () => this.snapshot;

  private set(change: Partial<BootSnapshot>) {
    this.snapshot = { ...this.snapshot, ...change };
    const current = this.snapshot.failure
      ? `failed:${this.snapshot.failure.kind}`
      : this.snapshot.synced
        ? "synced"
        : (this.snapshot.steps.find(s => s.state === "current")?.id ??
          "pending");
    // For the headless checks: where the boot is, without reading the page.
    document.documentElement.dataset.boot = this.snapshot.unlock
      ? "unlock"
      : current;
    document.documentElement.dataset.role = this.snapshot.role.kind;
    (globalThis as { __minawebBoot?: unknown }).__minawebBoot = {
      steps: this.snapshot.steps,
      failure: this.snapshot.failure,
      synced: this.snapshot.synced,
      frontierStore: this.snapshot.frontierStore,
      frontierStoreReset: this.snapshot.frontierStoreReset,
      sharing: this.snapshot.sharing,
      progress: this.snapshot.progress,
      startPart: this.snapshot.startPart,
      connect: this.snapshot.connect,
      role: this.snapshot.role.kind,
    };
    for (const listener of this.listeners) listener();
  }

  private step(id: StepId, change: Partial<Step>) {
    const now = Date.now();
    this.set({
      steps: this.snapshot.steps.map(s => {
        if (s.id !== id) return s;
        const next = { ...s, ...change };
        if (change.state === "current" && s.startedAt === null) {
          next.startedAt = now;
        }
        if (
          (change.state === "done" || change.state === "skipped") &&
          s.endedAt === null
        ) {
          next.endedAt = now;
          next.startedAt ??= now;
        }
        return next;
      }),
    });
  }

  private stepState(id: StepId) {
    return this.snapshot.steps.find(s => s.id === id)!;
  }

  private fail(id: StepId, failure: BootFailure) {
    this.step(id, { state: "stopped" });
    this.set({ failure });
    console.error(`minaweb: ${failure.title}: ${failure.detail}`);
  }

  /** The carried key the page took at its load (autostart.ts loadGate),
   * for the boot to hold instead of taking it again. */
  useCarried(carried: Promise<HeldKey | null> | null) {
    this.carried = carried;
  }

  /** Runs the boot, once. */
  start() {
    if (this.started) return;
    this.started = true;
    void this.run().catch(error => {
      const failure =
        error instanceof BootError
          ? error.failure
          : {
              kind: "start" as const,
              title: "The node didn't start",
              detail: error instanceof Error ? error.message : String(error),
            };
      const at =
        this.snapshot.steps.find(s => s.state === "current")?.id ?? "start";
      this.fail(at, failure);
    });
  }

  private async run() {
    const { config } = this;
    // The key a role change carried across the reload, before anything
    // else: both its copies are deleted here, whether it comes back or not.
    // The page's load took it already if it saw one.
    const carried = await (this.carried ?? takeHandoff()).catch(() => null);
    if (carried) keyStore.hold(carried);

    this.step("check", { state: "current" });
    const problem = checkPage(pageFacts());
    if (problem) throw new BootError(problem);
    this.step("check", {
      state: "done",
      detail: "Cross-origin isolated, with WebAssembly threads.",
    });

    this.step("download", { state: "current" });
    // The glue, the WASM, the genesis config's check and the ICE server
    // lists, asked for together: none waits on another. Each is used, and
    // its failure shown, at the step that needs it, as when they were
    // asked for one after the other.
    const base = location.href;
    const genesisUrl = new URL(config.genesisUrl, base).href;
    const stopDownload = new AbortController();
    const began = Date.now();
    const download = early(
      downloadWasm(
        config.wasm,
        download => {
          this.set({ download });
          this.step("download", {
            detail:
              download.total !== null
                ? `${formatBytes(download.received)} of ${formatBytes(download.total)}`
                : formatBytes(download.received),
          });
        },
        input => fetch(input, { signal: stopDownload.signal }),
      ),
    );
    const genesisChecked = early(this.checkFile(genesisUrl, "genesis config"));
    const iceLoaded = early(loadEachIceServers(config.iceServersUrls, base));
    let glue: WebNodeGlue;
    try {
      glue = await this.loadGlue();
    } catch (error) {
      stopDownload.abort();
      throw error;
    }
    this.glueRef = glue;
    const bytes = await download;
    const took = seconds(Date.now() - began);
    const downloaded = (compressed: number | null) =>
      `${formatBytes(bytes.byteLength)}${compressed !== null ? ` (${formatBytes(compressed)} compressed)` : ""} in ${took}, SHA-256 checked.`;
    this.step("download", { state: "done", detail: downloaded(null) });
    // The browser may add the download's resource timing a moment after
    // the body is read: look for it now and twice more.
    for (const ms of [0, 500, 2_000]) {
      setTimeout(() => {
        const compressed = compressedSize(config.wasm.url, bytes.byteLength);
        if (compressed !== null) {
          this.step("download", { detail: downloaded(compressed) });
        }
      }, ms);
    }

    this.set({ startPart: 0 });
    this.step("start", {
      state: "current",
      detail: `Compiling ${formatBytes(bytes.byteLength)} of WebAssembly.`,
    });
    // The pinned WASM, for the node's prover host if it ever starts one.
    (globalThis as { __MINA_WEBNODE_WASM__?: unknown }).__MINA_WEBNODE_WASM__ =
      Object.freeze({ url: config.wasm.url, sha256: config.wasm.sha256 });
    let memory: WebAssembly.Memory;
    try {
      // 64 KiB pages: 11 MiB to start, 4 GiB at most, as the node is linked.
      memory = new WebAssembly.Memory({
        initial: 176,
        maximum: 65536,
        shared: true,
      });
    } catch (error) {
      throw new BootError({
        kind: "load",
        title: "The browser couldn't give the node memory",
        detail: error instanceof Error ? error.message : String(error),
        hint: "Close other tabs, or use a device with more memory.",
      });
    }
    try {
      await glue.default({ module_or_path: bytes, memory });
    } catch (error) {
      throw new BootError({
        kind: "load",
        title: "This browser couldn't load the node",
        detail: error instanceof Error ? error.message : String(error),
        hint: "A current desktop browser with a few GB of free memory runs it.",
      });
    }
    // The take checked the carried key's format only; the WASM can now
    // check that it derives its public key.
    const derive = glue.account_public_key;
    if (carried && derive) {
      keyStore.dropIfMismatched({ account_public_key: k => derive(k) });
    }

    const roleArgs = await this.roleArguments();

    this.set({ startPart: 1 });
    this.step("start", { detail: "Fetching the genesis config and peers." });
    await genesisChecked;
    // A list from each relay's credential service, joined, each admitted only
    // if it meets the node's renewal contract; none: the node's built-in STUN
    // servers.
    const iceLists = admitIceLists(await iceLoaded);
    const iceServers = iceLists.some(Boolean) ? joinIceServers(iceLists) : null;
    if (config.iceServersUrls.length && glue.update_ice_servers) {
      const stopRenewal = renewEachIceServers(
        config.iceServersUrls,
        base,
        iceLists,
        servers => glue.update_ice_servers!(servers),
      );
      window.addEventListener("pagehide", event => {
        if (!event.persisted) stopRenewal();
      });
    }
    const frontierStore = frontierStoreChoice(
      config.frontierStore,
      safeStorage("webnodeFrontierStore"),
    );
    const resetAsked = takeResetRequest();
    const frontierStoreReset = frontierStore && resetAsked;
    const sharing = readSharing();
    this.set({ frontierStore, frontierStoreReset, sharing });

    this.set({ startPart: 2 });
    const role = this.snapshot.role;
    this.step("start", {
      detail:
        role.kind === "worker"
          ? "Building the verifier keys and the node, with its SNARK worker."
          : role.kind === "producer"
            ? "Building the verifier keys and the node, with its block producer."
            : "Building the verifier keys and the node.",
    });
    let rpc: WebNodeRpc;
    try {
      rpc = await glue.run(
        roleArgs.blockProducer,
        config.seedUrls.map(url => new URL(url, base).href),
        config.relays,
        genesisUrl,
        config.network,
        roleArgs.coinbaseReceiver,
        roleArgs.snarker,
        roleArgs.snarkerFee,
        runOptions(frontierStore, iceServers, frontierStoreReset, sharing),
      );
    } catch (error) {
      throw new BootError({
        kind: "start",
        title: "The node didn't start",
        detail: error instanceof Error ? error.message : String(error),
      });
    }
    const client = createNodeClient({
      transport: createWebNodeTransport(rpc, {
        buildEnv: () => glue.build_env(),
      }),
    });
    this.set({ client, glue, rpc });
    this.set({ startPart: START_PARTS.length });
    this.step("start", {
      state: "done",
      // The step's time is on its right: one duration a step.
      detail: "Running.",
    });
    (globalThis as { webnode?: unknown }).webnode = rpc;
    await this.follow(client);
  }

  /**
   * `run`'s role arguments. A producer's key is the one the tab held
   * before the restart (handoff.ts). Where the browser couldn't carry it
   * that way, it crossed encrypted with a passphrase, in this tab's
   * sessionStorage: the boot waits here for the passphrase, then deletes
   * the record. Without either, or if the visitor declines, the node starts
   * without a role, and the record stays for the shell to offer.
   */
  private async roleArguments(): Promise<RunRoleArgs> {
    const role = this.snapshot.role;
    if (role.kind !== "producer") return runRoleArgs(role, null);
    const held = keyStore.getSnapshot().key;
    if (held?.publicKey === role.publicKey) return runRoleArgs(role, held);
    const record = readStored("session", HANDOFF_KEY);
    let key: HeldKey | null = null;
    if (record && record.publicKey === role.publicKey) {
      this.step("start", {
        detail: "Waiting for the producer key's passphrase.",
      });
      this.set({
        unlock: { publicKey: role.publicKey, error: null, busy: false },
      });
      key = await new Promise<HeldKey | null>(resolve => {
        this.keyWaiter = resolve;
      });
      this.keyWaiter = null;
      this.set({ unlock: null });
    }
    if (!key) {
      // Started without producing: the encrypted copy stays, and the shell
      // offers to unlock it (app/carried-key.tsx), so the key isn't lost.
      writeRole(VIEWER);
      this.set({ role: VIEWER });
      return runRoleArgs(VIEWER, null);
    }
    removeStored("session", HANDOFF_KEY);
    keyStore.hold(key);
    return runRoleArgs(role, key);
  }

  /** The passphrase of the producer's key, at a producer's restart. */
  unlockProducer = async (passphrase: string) => {
    const unlock = this.snapshot.unlock;
    const record = readStored("session", HANDOFF_KEY);
    if (!unlock || !this.keyWaiter) return;
    if (!record) return this.keyWaiter(null);
    this.set({ unlock: { ...unlock, busy: true, error: null } });
    try {
      const check = this.glueRef?.account_public_key;
      const key = await decryptKey(
        record,
        passphrase,
        check ? { account_public_key: k => check(k) } : undefined,
      );
      this.keyWaiter?.(key);
    } catch (error) {
      this.set({
        unlock: {
          ...unlock,
          busy: false,
          error: error instanceof Error ? error.message : "Wrong passphrase.",
        },
      });
    }
  };

  /** Starts the node without the producer, at its restart. */
  startWithoutRole = () => {
    this.keyWaiter?.(null);
  };

  private async loadGlue(): Promise<WebNodeGlue> {
    const url = new URL(this.config.glueUrl, location.href).href;
    try {
      return (await import(/* @vite-ignore */ url)) as WebNodeGlue;
    } catch (error) {
      // Say whether the file is missing, or there but broken.
      const status = await fetch(url, { method: "HEAD" })
        .then(r => r.status)
        .catch(() => null);
      throw new BootError(
        status !== null && status !== 200
          ? {
              kind: "files",
              title: "The node's files aren't on this server",
              detail: `${url} answered HTTP ${status}.`,
            }
          : {
              kind: "load",
              title: "The node's script didn't load",
              detail: error instanceof Error ? error.message : String(error),
            },
      );
    }
  }

  private async checkFile(url: string, what: string) {
    let status: number | null = null;
    try {
      status = (await fetch(url, { method: "HEAD" })).status;
    } catch {
      status = null;
    }
    if (status !== 200) {
      throw new BootError({
        kind: "files",
        title: `The ${what} isn't on this server`,
        detail:
          status === null
            ? `No answer for ${url}.`
            : `${url} answered HTTP ${status}.`,
      });
    }
  }

  /** Polls the running node until it is Synced, moving the steps. */
  private async follow(client: NodeClient) {
    const connectFrom = Date.now();
    this.set({
      connect: {
        relays: this.config.relays.length,
        connecting: 0,
        connected: 0,
        since: connectFrom,
      },
    });
    this.step("connect", { state: "current", detail: "Dialling the relay." });
    for (;;) {
      const hasPeer = this.stepState("connect").state === "done";
      await new Promise(resolve =>
        setTimeout(resolve, pollDelay(hasPeer, Date.now() - connectFrom)),
      );
      let status;
      try {
        status = await client.status();
      } catch {
        // A busy node thread; the shell says so once the pages show.
        continue;
      }
      const connected = status.peers.filter(p => p.connected).length;
      if (this.stepState("connect").state !== "done") {
        this.set({
          connect: {
            relays: this.config.relays.length,
            connecting: status.peers.filter(p => p.status === "Connecting")
              .length,
            connected,
            since: connectFrom,
          },
        });
        if (connected > 0) {
          this.set({ failure: null });
          this.step("connect", {
            state: "done",
            detail: `${connected} connected.`,
          });
        } else {
          const waited = Date.now() - connectFrom;
          const connecting = status.peers.filter(
            p => p.status === "Connecting",
          ).length;
          this.step("connect", {
            detail: connecting
              ? `Handshaking with ${connecting}.`
              : "Dialling the relay.",
          });
          if (waited > PEER_WAIT_MS && !this.snapshot.failure) {
            this.set({
              failure: noPeerFailure(
                this.config.relays,
                location.origin,
                status.peers,
              ),
            });
          }
          continue;
        }
      }

      const phase = status.phase;
      if (phase === "Synced") {
        for (const id of ["ledgers", "blocks"] as const) {
          if (this.stepState(id).state !== "done") {
            this.step(id, {
              state:
                this.stepState(id).state === "pending" ? "skipped" : "done",
            });
          }
        }
        this.step("blocks", {
          detail: `At ${status.bestTip ? formatHeight(status.bestTip.height) : "the tip"}.`,
        });
        this.set({ synced: true, progress: null });
        return;
      }
      if (phase !== "Bootstrap" && phase !== "Catchup") continue;

      const [ledgers, blocks, restore, progress] = await Promise.all([
        client.ledgerSync().catch(() => null),
        client.syncBlocks().catch(() => null),
        client.syncRestore().catch(() => null),
        client.syncProgress().catch(() => null),
      ]);
      if (progress) {
        this.set({
          progress,
          ledgerMemory: rememberLedgers(this.snapshot.ledgerMemory, progress),
        });
      }
      // The node's sync stage says when the blocks start. Without it (an
      // older node), the sync record's blocks, which it lists as soon as it
      // has a target, before the ledgers.
      const blocksStarted = progress?.stage
        ? progress.stage === "Blocks" || progress.stage === "Commit"
        : phase === "Catchup" || (blocks !== null && blocks.total > 0);
      if (!blocksStarted) {
        this.set({
          steps: syncSteps(this.snapshot.steps, "ledgers", Date.now()),
        });
        this.step("ledgers", {
          detail: progress
            ? ledgerStepSentence(progress)
            : ledgers
              ? ledgerDetail(ledgers)
              : null,
        });
        continue;
      }
      if (this.stepState("ledgers").state !== "done") {
        this.set({
          steps: syncSteps(this.snapshot.steps, "blocks", Date.now()),
        });
        this.step("ledgers", {
          detail: restore
            ? "From this browser's store."
            : "Fetched from peers.",
        });
      }
      const strip = progress ? blockStrip(progress) : null;
      this.set({ steps: syncSteps(this.snapshot.steps, "blocks", Date.now()) });
      this.step("blocks", {
        detail:
          strip && !strip.waiting
            ? stripSentence(strip)
            : blocks && blocks.total > 0
              ? `${formatCount(blocks.applied)} of ${formatCount(blocks.total)} applied.`
              : status.target
                ? `To #${formatCount(status.target.height)}.`
                : "Catching up to the tip.",
      });
    }
  }
}

/** `work`, started now and awaited later: a failure is thrown where it is
 * awaited, and not reported as unhandled before that. */
function early<T>(work: Promise<T>): Promise<T> {
  work.catch(() => {});
  return work;
}

function safeStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
