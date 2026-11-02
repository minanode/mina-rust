import { describe, expect, test, vi } from "vitest";

import { createNodeClient } from "./client";
import { isUnreachable, kindOf } from "./errors";
import { ledgerTotal } from "./progress";
import { receiptOutcome } from "./verification";
import { createWebNodeTransport, type WebNodeRpc } from "./webnode";
import { STOP_CHANGE } from "./worker";
import statusFixture from "./fixtures/status-devnet.json";
import syncFixture from "./fixtures/stats-sync-devnet.json";
import verificationFixture from "./fixtures/verification-devnet.json";
import includedFixture from "./fixtures/included-devnet-3600.json";

/** A stand-in for the WASM's RpcSender, answering with recorded JSON. */
function fakeRpc(overrides: Partial<Record<string, unknown>> = {}) {
  const calls: string[] = [];
  const answer = (name: string, value: unknown) =>
    vi.fn(async (...args: unknown[]) => {
      calls.push(`${name}(${args.join(",")})`);
      if (value instanceof Error) throw value;
      return value;
    });
  const pick = (name: string, value: unknown) =>
    name in overrides ? overrides[name] : value;
  const rpc: WebNodeRpc = {
    status: answer("status", pick("status", statusFixture.status)),
    state: () => ({
      peers: answer("peers", pick("peers", statusFixture.status.peers)),
      message_progress: answer(
        "message_progress",
        pick("message_progress", {
          messages_stats: {},
          staking_ledger_sync: { fetched: 120_000, estimation: 240_000 },
          next_epoch_ledger_sync: { fetched: 240_000, estimation: 240_000 },
          root_ledger_sync: null,
        }),
      ),
    }),
    stats: () => ({
      sync: answer("sync", pick("sync", syncFixture.records)),
      block_producer: answer("block_producer", null),
      verification: answer(
        "verification",
        pick("verification", verificationFixture),
      ),
    }),
    transaction_pool: () => ({ get: answer("pool", []) }),
  };
  return { rpc, calls };
}

const webClient = (rpc: WebNodeRpc, timeoutMs?: number) =>
  createNodeClient({
    transport: createWebNodeTransport(rpc, {
      buildEnv: () => ({ version: "0.20.0", git: { commit_hash: "abc" } }),
      timeoutMs,
    }),
  });

describe("a web node through the client", () => {
  test("the status and peers parse as they do over HTTP", async () => {
    const { rpc } = fakeRpc();
    const client = webClient(rpc);
    const status = await client.status();
    expect(status.phase).toBe("Synced");
    expect(status.bestTip?.height).toBe(578883);
    const peers = await client.peers();
    expect(peers[0]?.connected).toBe(true);
    expect((await client.buildInfo()).version).toBe("0.20.0");
    expect(client.baseUrl).toBe("webnode:");
  });

  test("query parameters reach the RPC", async () => {
    const { rpc, calls } = fakeRpc();
    const client = webClient(rpc);
    await client.syncRestore();
    await client.verification(3);
    await client.verification({ limit: 128, offset: 256 });
    expect(calls).toEqual([
      "sync(1)",
      "verification(3,)",
      "verification(128,256)",
    ]);
  });

  test("bootstrap progress: ledgers, then blocks", async () => {
    const { rpc } = fakeRpc();
    const client = webClient(rpc);
    const ledgers = await client.ledgerSync();
    expect(ledgers.stakingEpoch).toEqual({ done: 120_000, total: 240_000 });
    expect(ledgers.root).toBeNull();
    expect(ledgerTotal(ledgers)).toEqual({ done: 360_000, total: 480_000 });
    const blocks = await client.syncBlocks();
    expect(blocks).toEqual({
      kind: "Catchup",
      total: 2,
      fetched: 2,
      applied: 2,
    });
  });

  test("verification receipts, newest first", async () => {
    const { rpc } = fakeRpc();
    const v = await webClient(rpc).verification(3);
    expect(v.receipts).toHaveLength(3);
    const [first] = v.receipts;
    expect(first.height).toBe(v.receipts[0].height);
    expect(first.receivedAtMs).toBeGreaterThan(1_700_000_000_000);
    expect(receiptOutcome(first)).toBe("passed");
    expect(v.medianProofTimeMs).toBeGreaterThan(0);
    expect(v.lastVerified?.proof.status).toBe("passed");
  });

  test("routes a web node hasn't got are absent, with the reason", async () => {
    const { rpc } = fakeRpc();
    const client = webClient(rpc);
    const graphql = await client.bestChain().catch((e: unknown) => e);
    expect(graphql).toMatchObject({
      kind: "absent",
      message: "The web node has no GraphQL API.",
    });
    const worker = await client.workerControl().catch((e: unknown) => e);
    expect(kindOf(worker)).toBe("absent");
    expect(isUnreachable(worker)).toBe(false);
  });

  test("a web node with roles reports its SNARK worker, and refuses a change", async () => {
    const { rpc } = fakeRpc();
    const control = {
      allowed: false,
      enabled: true,
      public_key: "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV",
      fee: 1_000_000_000,
      account_creation_fee: 1_000_000_000,
      worker: { kind: "Idle" },
      job: null,
      recent: [],
      paused_for_block: false,
      best_tip_height: 578883,
      zkapp_jobs: 0,
      proofs_made: 0,
      strategy: "Random",
      account_exists: false,
      fee_problem: null,
    };
    rpc.roles = () => ({
      snarker_control: async () => control,
      snarker_included: async () => includedFixture,
    });
    const client = webClient(rpc);
    const c = await client.workerControl();
    expect(c.enabled).toBe(true);
    expect(c.fee).toBe(1_000_000_000);
    expect(c.accountExists).toBe(false);
    const included = await client.includedWork();
    expect(included.entries.length).toBe(includedFixture.entries.length);
    const change = await client
      .changeWorker(STOP_CHANGE)
      .catch((e: unknown) => e);
    expect(change).toMatchObject({ kind: "absent" });
    expect(String((change as Error).message)).toMatch(
      /starts and stops with the node/,
    );
  });

  test("no status means a stopped node, not a pending one", async () => {
    const { rpc } = fakeRpc({ status: null });
    const error = await webClient(rpc)
      .status()
      .catch((e: unknown) => e);
    expect(error).toMatchObject({ kind: "failed" });
    expect(isUnreachable(error)).toBe(true);
  });

  test("a WASM error says what failed", async () => {
    const { rpc } = fakeRpc({ peers: new Error("RuntimeError: unreachable") });
    const error = await webClient(rpc)
      .peers()
      .catch((e: unknown) => e);
    expect(error).toMatchObject({
      kind: "failed",
      message: "The web node failed: RuntimeError: unreachable",
    });
  });

  test("a busy node times out as offline; an abort is the caller's", async () => {
    const never = () => new Promise<unknown>(() => {});
    const { rpc } = fakeRpc();
    rpc.status = never;
    const timedOut = await webClient(rpc, 20)
      .status()
      .catch((e: unknown) => e);
    expect(timedOut).toMatchObject({
      kind: "offline",
      message: "The web node didn't answer in 0 s.",
    });
    const controller = new AbortController();
    const pending = webClient(rpc).status(controller.signal);
    controller.abort();
    const aborted = await pending.catch((e: unknown) => e);
    expect(aborted).toBeInstanceOf(DOMException);
  });
});
