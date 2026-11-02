import { describe, expect, test, vi } from "vitest";

import {
  MINA_TOKEN_ID,
  parseBestChainHeaders,
  parseBlockDetail,
  parseLedgerAccounts,
  parseSyncProgress,
} from "./chainview";
import { commandFees } from "./chain";
import { createNodeClient } from "./client";
import { kindOf } from "./errors";
import { createWebNodeTransport, type WebNodeRpc } from "./webnode";

// Field names and shapes as crates/node/src/rpc/chain_view.rs serialises
// them (its tests pin the JSON keys).
const PRODUCER = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
const header = (height: number, hash: string) => ({
  height,
  hash,
  pred_hash: "3NKxUSAJE3wqJkrtBhMYhwzrMq3B5sKjPJQRyXz1YrPWA7761opD",
  global_slot: 7140 * 3 + 12,
  global_slot_since_genesis: 7140 * 3 + 12,
  epoch: 3,
  slot: 12,
  timestamp_ms: 1_793_000_000_000,
  producer: PRODUCER,
  coinbase_receiver: PRODUCER,
  user_commands: 2,
  zkapp_commands: 1,
  failed_commands: 1,
  snark_jobs: 1,
  // Amounts come as decimal strings; numbers parse too.
  transaction_fees: "30000000",
  snark_fees: 4_000_000,
  proof_emitted: true,
});
const TIP = "3NLtipHashxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

describe("best-chain headers", () => {
  test("parse newest first, with fees as bigints", () => {
    const parsed = parseBestChainHeaders({
      k: 290,
      root_height: 100,
      best_tip_height: 390,
      headers: [header(389, "3NLa"), header(390, TIP)],
    });
    expect(parsed.k).toBe(290);
    expect(parsed.headers.map(h => h.height)).toEqual([390, 389]);
    const h = parsed.headers[0];
    expect(h.stateHash).toBe(TIP);
    expect(h.producer).toBe(PRODUCER);
    expect(h.transactionFees).toBe(30_000_000n);
    expect(h.snarkFees).toBe(4_000_000n);
    expect(h.proofEmitted).toBe(true);
    expect(h.timeMs).toBe(1_793_000_000_000);
  });

  test("an empty or broken answer is no headers", () => {
    expect(parseBestChainHeaders(null).headers).toEqual([]);
    expect(
      parseBestChainHeaders({ headers: [{ height: 1 }, "x"] }).headers,
    ).toEqual([]);
  });
});

describe("a block's detail", () => {
  const raw = {
    header: header(390, TIP),
    snarked_ledger_hash: "jxsnarked",
    staged_ledger_hash: "jxstaged",
    coinbase: 720_000_000_000,
    commands: [
      {
        kind: "payment",
        hash: "5Jtx1",
        fee_payer: PRODUCER,
        receiver: "B62qreceiver",
        amount: 1_000_000_000,
        fee: 10_000_000,
        nonce: 4,
        memo: "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH",
        account_updates: null,
        failure: null,
      },
      {
        kind: "delegation",
        hash: "5Jtx2",
        fee_payer: PRODUCER,
        receiver: PRODUCER,
        amount: null,
        fee: 10_000_000,
        nonce: 5,
        memo: "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH",
        account_updates: null,
        failure: "Amount_insufficient_to_create_account",
      },
      {
        kind: "zkapp",
        hash: "5Jtx3",
        fee_payer: PRODUCER,
        receiver: null,
        amount: null,
        fee: 10_000_000,
        nonce: 6,
        memo: "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH",
        account_updates: 3,
        failure: null,
      },
    ],
    fee_transfers: [
      { receiver: PRODUCER, fee: 26_000_000, via_coinbase: false },
      { receiver: "B62qprover", fee: 4_000_000, via_coinbase: true },
    ],
    snark_work: [{ prover: "B62qprover", fee: 4_000_000, proofs: 2 }],
  };

  test("parses into the GraphQL block the pages show", () => {
    const b = parseBlockDetail(raw)!;
    expect(b.stateHash).toBe(TIP);
    expect(b.height).toBe(390);
    expect(b.creator).toBe(PRODUCER);
    expect(b.coinbase).toBe("720000000000");
    expect(b.userCommands.map(c => [c.kind, c.amount, c.failure])).toEqual([
      ["Payment", "1000000000", null],
      ["Delegation", null, "Amount_insufficient_to_create_account"],
    ]);
    expect(b.zkappCommands).toEqual([
      {
        hash: "5Jtx3",
        feePayer: PRODUCER,
        fee: "10000000",
        accountUpdates: 3,
        failed: false,
      },
    ]);
    expect(b.feeTransfers.map(f => f.type)).toEqual([
      "Fee_transfer",
      "Fee_transfer_via_coinbase",
    ]);
    expect(b.snarkJobs).toEqual([{ prover: "B62qprover", fee: "4000000" }]);
    expect(commandFees(b)).toBe(30_000_000n);
  });

  test("null is a block the frontier hasn't got", () => {
    expect(parseBlockDetail(null)).toBeNull();
  });
});

describe("sync progress", () => {
  test("ledgers and blocks", () => {
    const p = parseSyncProgress({
      k: 290,
      phase: "Bootstrap",
      stage: "Blocks",
      root_height: 100,
      target_height: 390,
      staking_epoch_ledger: { status: "done" },
      next_epoch_ledger: { status: "done" },
      root_snarked_ledger: {
        status: "done",
        fetched: null,
        total: null,
        accounts_fetched: null,
        accounts_total: null,
      },
      root_staged_ledger: { status: "done" },
      blocks: { total: 6, fetched: 4, applied: 2, failed: 0, cells: "AAaFf." },
    });
    expect(p.k).toBe(290);
    expect(p.stage).toBe("Blocks");
    expect(p.rootStaged.status).toBe("done");
    expect(p.blocks).toEqual({
      total: 6,
      fetched: 4,
      applied: 2,
      failed: 0,
      cells: "AAaFf.",
    });
  });

  test("a ledger that syncs, and unknown values", () => {
    const p = parseSyncProgress({
      stage: "Bogus",
      staking_epoch_ledger: {
        status: "syncing",
        fetched: 1000,
        total: 4000,
        accounts_fetched: 640,
        accounts_total: 3000,
      },
      next_epoch_ledger: { status: "nonsense" },
    });
    expect(p.stage).toBeNull();
    expect(p.blocks).toBeNull();
    expect(p.stakingEpoch).toEqual({
      status: "syncing",
      fetched: 1000,
      total: 4000,
      accountsFetched: 640,
      accountsTotal: 3000,
    });
    expect(p.nextEpoch.status).toBe("waiting");
  });
});

describe("ledger accounts", () => {
  const account = {
    public_key: PRODUCER,
    token_id: MINA_TOKEN_ID,
    token_symbol: "",
    balance: "9007199254740993000",
    nonce: 3,
    delegate: PRODUCER,
    timing: null,
    zkapp: null,
  };

  test("the MINA account first; a zkApp's state", () => {
    const parsed = parseLedgerAccounts([
      {
        ...account,
        token_id: "xToken",
        token_symbol: "TOK",
        zkapp: {
          app_state: ["1", "0", "0", "0", "0", "0", "0", "0"],
          verification_key_hash: "123",
          zkapp_version: 0,
          proved_state: true,
          zkapp_uri: "",
        },
      },
      account,
    ])!;
    expect(parsed.map(a => a.tokenId)).toEqual([MINA_TOKEN_ID, "xToken"]);
    expect(parsed[0].balance).toBe("9007199254740993000");
    expect(parsed[0].zkapp).toBeNull();
    expect(parsed[1].tokenSymbol).toBe("TOK");
    expect(parsed[1].zkapp?.appState[0]).toBe("1");
    expect(parsed[1].zkapp?.zkappUri).toBeNull();
  });

  test("empty is no account; null is no ledger yet", () => {
    expect(parseLedgerAccounts([])).toEqual([]);
    expect(parseLedgerAccounts(null)).toBeNull();
  });
});

describe("a web node answers the new routes", () => {
  function rpc(extra: Partial<WebNodeRpc> = {}) {
    const calls: string[] = [];
    const fn =
      (name: string, value: unknown) =>
      async (...args: unknown[]) => {
        calls.push(`${name}(${args.map(a => JSON.stringify(a)).join(",")})`);
        return value;
      };
    const node: WebNodeRpc = {
      status: fn("status", {}),
      state: () => ({
        peers: fn("peers", []),
        message_progress: fn("mp", {}),
      }),
      stats: () => ({
        sync: fn("sync", []),
        block_producer: fn("bp", null),
        verification: fn("verification", { receipts: [] }),
        sync_progress: fn("sync_progress", { k: 290, stage: "Synced" }),
      }),
      transaction_pool: () => ({ get: fn("pool", []) }),
      transition_frontier: () => ({
        best_chain: () => ({
          headers: fn("headers", { k: 290, headers: [header(390, TIP)] }),
          block: fn("block", null),
        }),
      }),
      ledger: () => ({
        latest: () => ({ accounts: () => ({ get: fn("get", []) }) }),
      }),
      ...extra,
    };
    return { node, calls };
  }
  const client = (node: WebNodeRpc) =>
    createNodeClient({ transport: createWebNodeTransport(node) });

  test("with the query passed through", async () => {
    const { node, calls } = rpc();
    const c = client(node);
    const headers = await c.bestChainHeaders({ limit: 50, maxHeight: 389 });
    expect(headers.headers[0].height).toBe(390);
    expect(await c.chainBlock(TIP)).toBeNull();
    expect((await c.syncProgress()).k).toBe(290);
    expect(await c.ledgerAccounts(PRODUCER)).toEqual([]);
    await c.verificationOf([TIP, "3NLb"]);
    expect(calls).toEqual([
      "headers(50,389)",
      `block("${TIP}")`,
      "sync_progress()",
      `get("${PRODUCER}")`,
      `verification(,,["${TIP}","3NLb"])`,
    ]);
  });

  test("an older web node says it can't", async () => {
    const { node } = rpc({ transition_frontier: undefined, ledger: undefined });
    const c = client(node);
    const failed = await c.chainBlock("5").catch(e => e);
    expect(kindOf(failed)).toBe("absent");
    expect(String(failed.message)).toMatch(/too old to show a block/);
    expect(kindOf(await c.ledgerAccounts(PRODUCER).catch(e => e))).toBe(
      "absent",
    );
    expect(kindOf(await c.bestChainHeaders().catch(e => e))).toBe("absent");
  });

  test("a block id goes through as one path segment", async () => {
    const block = vi.fn(async () => null);
    const { node } = rpc({
      transition_frontier: () => ({ best_chain: () => ({ block }) }),
    });
    await client(node).chainBlock(" 581331 ");
    expect(block).toHaveBeenCalledWith("581331");
  });
});
