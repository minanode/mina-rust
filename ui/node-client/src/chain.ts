/**
 * Blocks and accounts through the node's GraphQL, with the queries the
 * Angular app's chain pages send (frontend/src/app/features/chain).
 */

import { arr, bool, num, obj, str } from "./json";

export const BLOCK_FIELDS = `
  stateHash creator commandTransactionCount
  protocolState {
    previousStateHash
    consensusState { blockHeight slotSinceGenesis slot epoch }
    blockchainState { snarkedLedgerHash stagedLedgerHash date utcDate }
  }
  transactions {
    coinbase coinbaseReceiverAccount { publicKey }
    feeTransfer { recipient fee type }
    userCommands { hash from to amount fee memo nonce isDelegation failureReason }
    zkappCommands {
      hash failureReason { index failures }
      zkappCommand {
        feePayer { body { publicKey fee } }
        accountUpdates { body { publicKey callDepth } }
      }
    }
  }
  snarkJobs { prover fee }
`;

export const RECENT_BLOCKS = `query RecentBlocks($maxLength: Int!) { bestChain(maxLength: $maxLength) { ${BLOCK_FIELDS} } }`;
export const BLOCK_BY_HASH = `query BlockDetails($stateHash: String!) { block(stateHash: $stateHash) { ${BLOCK_FIELDS} } }`;
export const BLOCK_BY_HEIGHT = `query BlockAtHeight($height: Int!) { block(height: $height) { ${BLOCK_FIELDS} } }`;
export const ACCOUNT = `
  query LedgerAccount($publicKey: PublicKey!) {
    account(publicKey: $publicKey) {
      publicKey nonce
      balance { total liquid locked blockHeight stateHash }
      delegateAccount { publicKey }
      timing { initialMinimumBalance cliffTime cliffAmount vestingPeriod vestingIncrement }
    }
  }`;

/** Blocks the list shows. */
export const RECENT_BLOCKS_LENGTH = 20;

/** Slots per epoch on Mina mainnet and devnet. */
export const SLOTS_PER_EPOCH = 7140;

export interface UserCommand {
  hash: string | null;
  kind: "Payment" | "Delegation";
  from: string | null;
  to: string | null;
  /** Nanomina, as a decimal string. */
  amount: string | null;
  fee: string | null;
  memo: string | null;
  nonce: number | null;
  failure: string | null;
}

export interface ZkappCommand {
  hash: string | null;
  feePayer: string | null;
  fee: string | null;
  accountUpdates: number;
  failed: boolean;
}

export interface FeeTransfer {
  recipient: string | null;
  fee: string | null;
  type: string | null;
}

export interface Block {
  stateHash: string;
  previousStateHash: string | null;
  height: number | null;
  /** The slot since the hard fork: epoch × 7140 + slot. */
  slot: number | null;
  slotSinceGenesis: number | null;
  epoch: number | null;
  /** Block time, ms since the epoch. */
  timeMs: number | null;
  creator: string | null;
  coinbase: string | null;
  coinbaseReceiver: string | null;
  snarkedLedgerHash: string | null;
  stagedLedgerHash: string | null;
  userCommands: UserCommand[];
  zkappCommands: ZkappCommand[];
  feeTransfers: FeeTransfer[];
  snarkJobs: { prover: string | null; fee: string | null }[];
}

function amount(value: unknown): string | null {
  const s = str(value);
  return s !== null && /^\d+$/.test(s) ? s : null;
}

export function parseBlock(value: unknown): Block | null {
  const b = obj(value);
  const stateHash = str(b?.stateHash);
  if (!b || !stateHash) return null;
  const protocol = obj(b.protocolState) ?? {};
  const consensus = obj(protocol.consensusState) ?? {};
  const chain = obj(protocol.blockchainState) ?? {};
  const txs = obj(b.transactions) ?? {};
  const epoch = num(consensus.epoch);
  const slotInEpoch = num(consensus.slot);
  return {
    stateHash,
    previousStateHash: str(protocol.previousStateHash),
    height: num(consensus.blockHeight),
    slot:
      epoch !== null && slotInEpoch !== null
        ? epoch * SLOTS_PER_EPOCH + slotInEpoch
        : null,
    slotSinceGenesis: num(consensus.slotSinceGenesis),
    epoch,
    timeMs: num(chain.utcDate) ?? num(chain.date),
    creator: str(b.creator),
    coinbase: amount(txs.coinbase),
    coinbaseReceiver: str(obj(txs.coinbaseReceiverAccount)?.publicKey),
    snarkedLedgerHash: str(chain.snarkedLedgerHash),
    stagedLedgerHash: str(chain.stagedLedgerHash),
    userCommands: arr(txs.userCommands).map(value => {
      const c = obj(value) ?? {};
      return {
        hash: str(c.hash),
        kind: bool(c.isDelegation) ? "Delegation" : "Payment",
        from: str(c.from),
        to: str(c.to),
        amount: amount(c.amount),
        fee: amount(c.fee),
        memo: str(c.memo),
        nonce: num(c.nonce),
        failure: str(c.failureReason),
      } satisfies UserCommand;
    }),
    zkappCommands: arr(txs.zkappCommands).map(value => {
      const c = obj(value) ?? {};
      const command = obj(c.zkappCommand) ?? {};
      const feePayer = obj(obj(command.feePayer)?.body) ?? {};
      return {
        hash: str(c.hash),
        feePayer: str(feePayer.publicKey),
        fee: amount(feePayer.fee),
        accountUpdates: arr(command.accountUpdates).length,
        failed: arr(c.failureReason).length > 0,
      };
    }),
    feeTransfers: arr(txs.feeTransfer).map(value => {
      const f = obj(value) ?? {};
      return {
        recipient: str(f.recipient),
        fee: amount(f.fee),
        type: str(f.type),
      };
    }),
    snarkJobs: arr(b.snarkJobs).map(value => {
      const j = obj(value) ?? {};
      return { prover: str(j.prover), fee: amount(j.fee) };
    }),
  };
}

/** `bestChain` is oldest first; the list shows the newest first. */
export function parseBestChain(data: Record<string, unknown>): Block[] {
  return arr(data.bestChain)
    .map(parseBlock)
    .filter((b): b is Block => b !== null)
    .sort((a, b) => (b.height ?? 0) - (a.height ?? 0));
}

export function parseBlockAnswer(data: Record<string, unknown>): Block | null {
  return parseBlock(data.block);
}

/** The sum of a block's command fees (payments, delegations and zkApp fee
 * payers), in nanomina; null if any fee is unreadable. */
export function commandFees(block: Block): bigint | null {
  const fees = [
    ...block.userCommands.map(c => c.fee),
    ...block.zkappCommands.map(c => c.fee),
  ];
  if (fees.some(fee => fee === null)) return null;
  return fees.reduce((sum, fee) => sum + BigInt(fee as string), 0n);
}

export function blockCounts(block: Block) {
  const delegations = block.userCommands.filter(
    c => c.kind === "Delegation",
  ).length;
  return {
    payments: block.userCommands.length - delegations,
    delegations,
    zkapps: block.zkappCommands.length,
    works: block.snarkJobs.length,
  };
}

/** What the detail route's id is: a height or a state hash. */
export function blockQuery(
  id: string,
): { query: string; variables: Record<string, unknown> } | null {
  const trimmed = id.trim();
  if (/^\d{1,10}$/.test(trimmed)) {
    return { query: BLOCK_BY_HEIGHT, variables: { height: Number(trimmed) } };
  }
  if (/^3N[1-9A-HJ-NP-Za-km-z]{50}$/.test(trimmed)) {
    return { query: BLOCK_BY_HASH, variables: { stateHash: trimmed } };
  }
  return null;
}

export interface Account {
  publicKey: string;
  nonce: number | null;
  /** Nanomina. */
  balance: string | null;
  liquid: string | null;
  locked: string | null;
  balanceHeight: number | null;
  delegate: string | null;
  timing: {
    initialMinimumBalance: string | null;
    cliffTime: number | null;
    cliffAmount: string | null;
    vestingPeriod: number | null;
    vestingIncrement: string | null;
  };
}

/** Null when the ledger has no account for the key. */
export function parseAccount(data: Record<string, unknown>): Account | null {
  const a = obj(data.account);
  const publicKey = str(a?.publicKey);
  if (!a || !publicKey) return null;
  const balance = obj(a.balance) ?? {};
  const timing = obj(a.timing) ?? {};
  return {
    publicKey,
    nonce: num(a.nonce),
    balance: amount(balance.total),
    liquid: amount(balance.liquid),
    locked: amount(balance.locked),
    balanceHeight: num(balance.blockHeight),
    delegate: str(obj(a.delegateAccount)?.publicKey),
    timing: {
      initialMinimumBalance: amount(timing.initialMinimumBalance),
      cliffTime: num(timing.cliffTime),
      cliffAmount: amount(timing.cliffAmount),
      vestingPeriod: num(timing.vestingPeriod),
      vestingIncrement: amount(timing.vestingIncrement),
    },
  };
}

/** Timed accounts have an initial minimum balance above zero. */
export function isTimed(account: Account): boolean {
  const min = account.timing.initialMinimumBalance;
  return min !== null && BigInt(min) > 0n;
}
