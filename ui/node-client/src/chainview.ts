/**
 * MINAWEB-RPC1: the compact best-chain and sync reads
 * (crates/node/src/rpc/chain_view.rs). A native node serves them at
 * `GET /best-chain/headers`, `GET /best-chain/block/{id}` and
 * `GET /stats/sync/progress`; a web node answers the same JSON from its
 * `RpcSender`. A web node also answers `GET /accounts/{publicKey}` from its
 * ledger. A block's detail parses into the GraphQL `Block`, so the pages
 * that show one work with either source.
 */

import type {
  Account,
  Block,
  FeeTransfer,
  UserCommand,
  ZkappCommand,
} from "./chain";
import { arr, bool, num, obj, str } from "./json";

export interface BlockHeader {
  height: number;
  stateHash: string;
  previousStateHash: string | null;
  /** The slot since the hard fork. */
  slot: number | null;
  slotSinceGenesis: number | null;
  epoch: number | null;
  /** Block time, ms since the epoch. */
  timeMs: number | null;
  producer: string | null;
  coinbaseReceiver: string | null;
  /** Payments and delegations. */
  userCommands: number;
  zkappCommands: number;
  failedCommands: number;
  snarkJobs: number;
  /** Nanomina. */
  transactionFees: bigint | null;
  snarkFees: bigint | null;
  proofEmitted: boolean;
}

export interface BestChainHeaders {
  /** Consensus k: a synced frontier holds k + 1 blocks. */
  k: number | null;
  rootHeight: number | null;
  bestTipHeight: number | null;
  /** Newest first. */
  headers: BlockHeader[];
}

function nanomina(value: unknown): bigint | null {
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
    return BigInt(value);
  }
  const s = typeof value === "string" ? value : null;
  return s !== null && /^\d+$/.test(s) ? BigInt(s) : null;
}

function decimal(value: unknown): string | null {
  const n = nanomina(value);
  return n === null ? null : n.toString();
}

export function parseBlockHeader(value: unknown): BlockHeader | null {
  const h = obj(value);
  const height = num(h?.height);
  const stateHash = str(h?.hash);
  if (!h || height === null || !stateHash) return null;
  return {
    height,
    stateHash,
    previousStateHash: str(h.pred_hash),
    slot: num(h.global_slot),
    slotSinceGenesis: num(h.global_slot_since_genesis),
    epoch: num(h.epoch),
    timeMs: num(h.timestamp_ms),
    producer: str(h.producer),
    coinbaseReceiver: str(h.coinbase_receiver),
    userCommands: num(h.user_commands) ?? 0,
    zkappCommands: num(h.zkapp_commands) ?? 0,
    failedCommands: num(h.failed_commands) ?? 0,
    snarkJobs: num(h.snark_jobs) ?? 0,
    transactionFees: nanomina(h.transaction_fees),
    snarkFees: nanomina(h.snark_fees),
    proofEmitted: bool(h.proof_emitted) ?? false,
  };
}

export function parseBestChainHeaders(raw: unknown): BestChainHeaders {
  const r = obj(raw) ?? {};
  return {
    k: num(r.k),
    rootHeight: num(r.root_height),
    bestTipHeight: num(r.best_tip_height),
    headers: arr(r.headers)
      .map(parseBlockHeader)
      .filter((h): h is BlockHeader => h !== null)
      .sort((a, b) => b.height - a.height),
  };
}

/** A best-chain block's detail as the GraphQL `Block`, with its header. */
export interface BlockDetail extends Block {
  header: BlockHeader;
}

/** Null when the node's frontier hasn't got the block. */
export function parseBlockDetail(raw: unknown): BlockDetail | null {
  const d = obj(raw);
  const header = parseBlockHeader(d?.header);
  if (!d || !header) return null;
  const commands = arr(d.commands).map(value => obj(value) ?? {});
  const userCommands: UserCommand[] = commands
    .filter(c => c.kind === "payment" || c.kind === "delegation")
    .map(c => ({
      hash: str(c.hash),
      kind: c.kind === "delegation" ? "Delegation" : "Payment",
      from: str(c.fee_payer),
      to: str(c.receiver),
      amount: decimal(c.amount),
      fee: decimal(c.fee),
      memo: str(c.memo),
      nonce: num(c.nonce),
      failure: str(c.failure),
    }));
  const zkappCommands: ZkappCommand[] = commands
    .filter(c => c.kind === "zkapp")
    .map(c => ({
      hash: str(c.hash),
      feePayer: str(c.fee_payer),
      fee: decimal(c.fee),
      accountUpdates: num(c.account_updates) ?? 0,
      failed: str(c.failure) !== null,
    }));
  const feeTransfers: FeeTransfer[] = arr(d.fee_transfers).map(value => {
    const t = obj(value) ?? {};
    return {
      recipient: str(t.receiver),
      fee: decimal(t.fee),
      // GraphQL's names for the two kinds.
      type: bool(t.via_coinbase) ? "Fee_transfer_via_coinbase" : "Fee_transfer",
    };
  });
  return {
    header,
    stateHash: header.stateHash,
    previousStateHash: header.previousStateHash,
    height: header.height,
    slot: header.slot,
    slotSinceGenesis: header.slotSinceGenesis,
    epoch: header.epoch,
    timeMs: header.timeMs,
    creator: header.producer,
    coinbase: decimal(d.coinbase),
    coinbaseReceiver: header.coinbaseReceiver,
    snarkedLedgerHash: str(d.snarked_ledger_hash),
    stagedLedgerHash: str(d.staged_ledger_hash),
    userCommands,
    zkappCommands,
    feeTransfers,
    snarkJobs: arr(d.snark_work).map(value => {
      const w = obj(value) ?? {};
      return { prover: str(w.prover), fee: decimal(w.fee) };
    }),
  };
}

export type LedgerSyncStatus = "waiting" | "syncing" | "done";

export interface LedgerProgress {
  status: LedgerSyncStatus;
  /** What the bar fills with: accounts and hashes of an estimate for a
   * snarked ledger, parts for the staged one. */
  fetched: number | null;
  total: number | null;
  accountsFetched: number | null;
  accountsTotal: number | null;
}

export interface BlocksProgress {
  /** Blocks in the chain the sync builds, the root first. */
  total: number;
  fetched: number;
  applied: number;
  failed: number;
  /** One character a block: `.` missing, `f` fetching, `F` fetched, `a`
   * applying, `A` applied, `x` failed. */
  cells: string;
}

export type SyncStage =
  | "Idle"
  | "Init"
  | "StakingLedger"
  | "NextEpochLedger"
  | "RootLedger"
  | "Blocks"
  | "Commit"
  | "Synced";

export interface SyncProgress {
  k: number | null;
  phase: string | null;
  stage: SyncStage | null;
  rootHeight: number | null;
  targetHeight: number | null;
  stakingEpoch: LedgerProgress;
  nextEpoch: LedgerProgress;
  rootSnarked: LedgerProgress;
  rootStaged: LedgerProgress;
  blocks: BlocksProgress | null;
}

const STAGES = new Set<string>([
  "Idle",
  "Init",
  "StakingLedger",
  "NextEpochLedger",
  "RootLedger",
  "Blocks",
  "Commit",
  "Synced",
]);

function ledgerProgress(value: unknown): LedgerProgress {
  const l = obj(value) ?? {};
  const status = str(l.status);
  return {
    status: status === "syncing" || status === "done" ? status : "waiting",
    fetched: num(l.fetched),
    total: num(l.total),
    accountsFetched: num(l.accounts_fetched),
    accountsTotal: num(l.accounts_total),
  };
}

export function parseSyncProgress(raw: unknown): SyncProgress {
  const r = obj(raw) ?? {};
  const stage = str(r.stage);
  const b = obj(r.blocks);
  const cells = str(b?.cells);
  return {
    k: num(r.k),
    phase: str(r.phase),
    stage: stage && STAGES.has(stage) ? (stage as SyncStage) : null,
    rootHeight: num(r.root_height),
    targetHeight: num(r.target_height),
    stakingEpoch: ledgerProgress(r.staking_epoch_ledger),
    nextEpoch: ledgerProgress(r.next_epoch_ledger),
    rootSnarked: ledgerProgress(r.root_snarked_ledger),
    rootStaged: ledgerProgress(r.root_staged_ledger),
    blocks:
      b && cells !== null
        ? {
            total: num(b.total) ?? cells.length,
            fetched: num(b.fetched) ?? 0,
            applied: num(b.applied) ?? 0,
            failed: num(b.failed) ?? 0,
            cells,
          }
        : null,
  };
}

/** A ledger account as the web node's ledger answers it. */
export interface LedgerAccount extends Account {
  tokenId: string | null;
  tokenSymbol: string | null;
  zkapp: {
    appState: string[];
    verificationKeyHash: string | null;
    zkappVersion: number | null;
    provedState: boolean;
    zkappUri: string | null;
  } | null;
}

/** The default token's id. */
export const MINA_TOKEN_ID =
  "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf";

export function parseLedgerAccount(value: unknown): LedgerAccount | null {
  const a = obj(value);
  const publicKey = str(a?.public_key);
  if (!a || !publicKey) return null;
  const timing = obj(a.timing);
  const zkapp = obj(a.zkapp);
  const balance = decimal(a.balance);
  return {
    publicKey,
    nonce: num(a.nonce),
    balance,
    // The web node's ledger gives no liquid balance; the page shows the
    // vesting schedule instead.
    liquid: null,
    locked: null,
    balanceHeight: null,
    delegate: str(a.delegate),
    timing: {
      initialMinimumBalance: decimal(timing?.initial_minimum_balance),
      cliffTime: num(timing?.cliff_time),
      cliffAmount: decimal(timing?.cliff_amount),
      vestingPeriod: num(timing?.vesting_period),
      vestingIncrement: decimal(timing?.vesting_increment),
    },
    tokenId: str(a.token_id),
    tokenSymbol: str(a.token_symbol) || null,
    zkapp: zkapp
      ? {
          appState: arr(zkapp.app_state).map(v => str(v) ?? "0"),
          verificationKeyHash: str(zkapp.verification_key_hash),
          zkappVersion: num(zkapp.zkapp_version),
          provedState: bool(zkapp.proved_state) ?? false,
          zkappUri: str(zkapp.zkapp_uri) || null,
        }
      : null,
  };
}

/** A key's accounts, the MINA account first; null if the node gave none
 * (it hasn't got a ledger yet). */
export function parseLedgerAccounts(raw: unknown): LedgerAccount[] | null {
  if (raw === null || raw === undefined) return null;
  return arr(raw)
    .map(parseLedgerAccount)
    .filter((a): a is LedgerAccount => a !== null)
    .sort(
      (a, b) =>
        Number(b.tokenId === MINA_TOKEN_ID) -
        Number(a.tokenId === MINA_TOKEN_ID),
    );
}
