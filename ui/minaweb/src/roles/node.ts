/**
 * What the role screens ask the web node, through the WASM's
 * `rpc.roles()` (crates/node/common/src/service/rpc/roles.rs) and its
 * signer (`transaction_pool().inject().sign_and_inject_payment`), parsed
 * into plain values. Amounts are nanomina, as bigint where they are sums.
 */

import {
  arr,
  bool,
  num,
  obj,
  str,
  MINA_TOKEN_ID,
  parseLedgerAccounts,
} from "@minanode/node-client";

export interface RolesRpc {
  inclusion(hash: string, depth: number): Promise<unknown>;
  stake(publicKey: string): Promise<unknown>;
  consensus_constants(): Promise<unknown>;
  consensus_time(): Promise<unknown>;
}

export interface SignerRpc {
  sign_and_inject_payment(
    privateKey: string,
    payment: unknown,
  ): Promise<unknown>;
}

/** The parts of the WASM's RpcSender this file calls. */
export interface RoleNodeRpc {
  ledger?(): {
    latest(): { accounts(): { get?(publicKey: string): Promise<unknown> } };
  };
  roles?(): RolesRpc;
  transaction_pool(): { inject?(): SignerRpc; get(): Promise<unknown> };
}

export interface Account {
  publicKey: string;
  balance: bigint;
  nonce: number;
  delegate: string | null;
}

/**
 * Nanomina, exactly. The node sends amounts as decimal strings; a JSON
 * number is taken only while it is a safe integer, as a larger one has
 * already lost its low digits.
 */
export function big(value: unknown): bigint | null {
  if (typeof value === "string")
    return /^\d{1,20}$/.test(value) ? BigInt(value) : null;
  if (typeof value === "number") {
    return Number.isSafeInteger(value) && value >= 0 ? BigInt(value) : null;
  }
  return null;
}

export interface Inclusion {
  height: number;
  blockHash: string;
  depth: number;
  failure: string | null;
}

/** One look for a command: the tip it searched from, how many blocks it
 * searched, and where the command is (null: not in them). */
export interface InclusionLook {
  tipHeight: number;
  depthSearched: number;
  found: Inclusion | null;
}

export function parseInclusionLook(raw: unknown): InclusionLook | null {
  const l = obj(raw);
  const tipHeight = num(l?.tip_height);
  const depthSearched = num(l?.depth_searched);
  if (!l || tipHeight === null || depthSearched === null) return null;
  return { tipHeight, depthSearched, found: parseInclusion(l.found) };
}

export function parseInclusion(raw: unknown): Inclusion | null {
  const i = obj(raw);
  const height = num(i?.height);
  const blockHash = str(i?.block_hash);
  if (!i || height === null || !blockHash) return null;
  return {
    height,
    blockHash,
    depth: num(i.depth) ?? 0,
    failure: str(i.failure),
  };
}

export interface LedgerStake {
  hash: string;
  /** Null when the node couldn't read that ledger. */
  delegatedStake: bigint | null;
  delegators: number | null;
  totalCurrency: bigint;
}

export interface Stake {
  epoch: number;
  bestTipHeight: number;
  staking: LedgerStake;
  next: LedgerStake;
}

function ledgerStake(raw: unknown): LedgerStake | null {
  const l = obj(raw);
  const total = big(l?.total_currency);
  if (!l || total === null) return null;
  return {
    hash: str(l.hash) ?? "",
    delegatedStake: big(l.delegated_stake),
    delegators: num(l.delegators),
    totalCurrency: total,
  };
}

export function parseStake(raw: unknown): Stake | null {
  const s = obj(raw);
  const epoch = num(s?.epoch);
  const staking = ledgerStake(s?.staking);
  const next = ledgerStake(s?.next);
  if (!s || epoch === null || !staking || !next) return null;
  return { epoch, bestTipHeight: num(s.best_tip_height) ?? 0, staking, next };
}

export interface Consensus {
  slotsPerEpoch: number;
  slotMs: number;
  /** Blocks on top of a block before it is final (`k`). */
  k: number;
}

export function parseConsensus(raw: unknown): Consensus | null {
  const c = obj(raw);
  const slotsPerEpoch = num(c?.slots_per_epoch);
  const slotMs = num(c?.slot_duration_ms);
  if (!c || !slotsPerEpoch || !slotMs) return null;
  return { slotsPerEpoch, slotMs, k: num(c.k) ?? 290 };
}

export interface ConsensusTime {
  epoch: number;
  globalSlot: number;
  /** The slot in its epoch. */
  slot: number;
  /** When the current slot started, ms since the Unix epoch. */
  slotStartMs: number;
}

/** Timestamps arrive as nanoseconds, as a number or a string. */
function nanosToMs(value: unknown): number | null {
  // Nanoseconds overflow a safe integer, but a millisecond is far above
  // the precision a JSON number keeps.
  if (typeof value === "number") {
    return Number.isFinite(value) && value >= 0
      ? Math.floor(value / 1e6)
      : null;
  }
  const n = big(value);
  return n === null ? null : Number(n / 1_000_000n);
}

export function parseConsensusTime(raw: unknown): ConsensusTime | null {
  const t = obj(raw);
  const epoch = num(t?.epoch);
  const globalSlot = num(t?.global_slot);
  const slot = num(t?.slot);
  const start = nanosToMs(t?.start_time);
  if (!t || epoch === null || globalSlot === null || slot === null) return null;
  if (start === null) return null;
  return { epoch, globalSlot, slot, slotStartMs: start };
}

export interface SendReceipt {
  hash: string;
  accepted: boolean;
  /** Why the pool refused it, in the node's words. */
  rejection: string | null;
}

/** The node's inject response: `{ hash, accepted, response }`. The
 * response is untagged: the accepted commands, `[command, error]` pairs
 * for a refusal, or error strings for a failure. */
export function parseSendReceipt(raw: unknown): SendReceipt {
  const r = obj(raw);
  const hash = str(r?.hash);
  if (!r || !hash) throw new Error("The node gave no transaction hash.");
  const accepted = bool(r.accepted) ?? false;
  let rejection: string | null = null;
  if (!accepted) {
    const reasons = arr(r.response)
      .map(entry => {
        if (typeof entry === "string") return entry;
        const pair = arr(entry);
        const reason = pair.length === 2 ? pair[1] : entry;
        return typeof reason === "string" ? reason : JSON.stringify(reason);
      })
      .filter(Boolean);
    rejection = reasons.length
      ? reasons.join("; ")
      : "The pool took nothing, and gave no reason.";
  }
  return { hash, accepted, rejection };
}

/** Whether the pool holds a command with this hash. */
export function poolHas(raw: unknown, hash: string): boolean {
  return arr(raw).some(entry => str(obj(entry)?.hash) === hash);
}

export class RoleNode {
  constructor(private readonly rpc: RoleNodeRpc) {}

  /** The web node has the role RPCs (MINAWEB-P2-WRITE or later). */
  get available(): boolean {
    return typeof this.rpc.roles === "function";
  }

  private roles(): RolesRpc {
    if (typeof this.rpc.roles !== "function") {
      throw new Error(
        "This web node is older than the page: it has no key or role screens.",
      );
    }
    return this.rpc.roles();
  }

  async account(publicKey: string): Promise<Account | null> {
    const accounts = this.rpc.ledger?.().latest().accounts();
    if (!accounts?.get)
      throw new Error("This web node cannot look up accounts.");
    const list = parseLedgerAccounts(await accounts.get(publicKey));
    if (list === null) throw new Error("The node has no ledger to read yet.");
    const account = list.find(
      a => a.tokenId === MINA_TOKEN_ID && a.publicKey === publicKey,
    );
    if (!account) return null;
    if (
      account.balance === null ||
      account.nonce === null ||
      !Number.isInteger(account.nonce) ||
      account.nonce < 0 ||
      account.nonce > 0xffff_ffff
    ) {
      throw new Error("The node returned an invalid balance or nonce.");
    }
    return {
      publicKey: account.publicKey,
      balance: BigInt(account.balance),
      nonce: account.nonce,
      delegate: account.delegate,
    };
  }

  async stake(publicKey: string): Promise<Stake | null> {
    return parseStake(await this.roles().stake(publicKey));
  }

  /** Null when the node has no best tip yet: nothing is known. */
  async inclusion(hash: string, depth: number): Promise<InclusionLook | null> {
    return parseInclusionLook(await this.roles().inclusion(hash, depth));
  }

  async consensus(): Promise<Consensus | null> {
    return parseConsensus(await this.roles().consensus_constants());
  }

  async consensusTime(): Promise<ConsensusTime | null> {
    return parseConsensusTime(await this.roles().consensus_time());
  }

  async inPool(hash: string): Promise<boolean> {
    return poolHas(await this.rpc.transaction_pool().get(), hash);
  }

  /** Signs in the WASM and submits to this node's pool. */
  async sendPayment(
    privateKey: string,
    payment: {
      from: string;
      to: string;
      amount: bigint;
      fee: bigint;
      nonce: number;
      memo: string;
    },
  ): Promise<SendReceipt> {
    const inject = this.rpc.transaction_pool().inject?.();
    if (!inject) throw new Error("This web node can't sign transactions.");
    let raw: unknown;
    try {
      raw = await inject.sign_and_inject_payment(privateKey, {
        from: payment.from,
        to: payment.to,
        amount: payment.amount.toString(),
        fee: payment.fee.toString(),
        nonce: payment.nonce,
        memo: payment.memo,
        valid_until: null,
      });
    } catch (error) {
      // The signer's errors name the field, never the key.
      throw new Error(
        `The node didn't sign it: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    return parseSendReceipt(raw);
  }
}
