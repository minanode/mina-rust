/**
 * A payment from the page's key: the form's checks, and what became of it
 * once sent. Amounts are nanomina (bigint); the form takes MINA.
 */

import { minaToNanomina } from "@minanode/node-client";

import type { Account, Inclusion, InclusionLook } from "./node";

/** OCaml's `minimum_user_command_fee`: 0.001 MINA. */
export const MIN_FEE = 1_000_000n;
/** The fee the form starts with: 0.1 MINA, as the OCaml client's default. */
export const DEFAULT_FEE = "0.1";
/** A payment to a key with no account pays this much of its amount to
 * create the account (`account_creation_fee`, 1 MINA). */
export const ACCOUNT_CREATION_FEE = 1_000_000_000n;
/** A memo's bytes. */
export const MEMO_BYTES = 32;
/** Mina's recommended wait for a payment: blocks on top of its block. */
export const CONFIRMATIONS = 15;

export interface PaymentForm {
  to: string;
  amount: string;
  fee: string;
  memo: string;
}

export interface Payment {
  to: string;
  amount: bigint;
  fee: bigint;
  memo: string;
}

export type FormErrors = Partial<Record<keyof PaymentForm | "form", string>>;

export interface CheckContext {
  from: string;
  /** The sender's account, null if it doesn't exist. */
  sender: Account | null;
  /** The recipient's account: null if it doesn't exist, undefined if not
   * looked up yet. */
  recipient?: Account | null;
  validKey: (publicKey: string) => boolean;
}

function mina(text: string): bigint | null {
  const t = text.trim();
  if (!/^\d+(\.\d{1,9})?$/.test(t)) return null;
  return minaToNanomina(t);
}

export function memoBytes(memo: string): number {
  return new TextEncoder().encode(memo).length;
}

/** The form's problems, field by field, or the payment it makes. */
export function checkPayment(
  form: PaymentForm,
  ctx: CheckContext,
): { payment: Payment | null; errors: FormErrors } {
  const errors: FormErrors = {};
  const to = form.to.trim();
  if (!to) errors.to = "Enter the recipient's public key (B62…).";
  else if (!ctx.validKey(to)) errors.to = "That isn't a valid public key.";
  else if (to === ctx.from) errors.to = "That is this key: send to another.";

  const amount = mina(form.amount);
  if (amount === null) errors.amount = "An amount in MINA, up to 9 decimals.";
  else if (amount <= 0n) errors.amount = "More than 0.";
  else if (ctx.recipient === null && amount < ACCOUNT_CREATION_FEE) {
    errors.amount =
      "The recipient has no account yet: send at least 1 MINA, which creates it.";
  }

  const fee = mina(form.fee);
  if (fee === null) errors.fee = "A fee in MINA, up to 9 decimals.";
  else if (fee < MIN_FEE) errors.fee = "At least 0.001 MINA.";

  if (memoBytes(form.memo) > MEMO_BYTES) {
    errors.memo = `At most ${MEMO_BYTES} bytes.`;
  }

  if (!ctx.sender) {
    errors.form =
      "This key has no account on devnet yet. Fund it from the faucet, then send.";
  } else if (amount !== null && fee !== null) {
    if (amount + fee > ctx.sender.balance) {
      errors.amount = "More than this key's balance, with the fee.";
    }
  }

  const ok = Object.keys(errors).length === 0;
  return {
    payment:
      ok && amount !== null && fee !== null
        ? { to, amount, fee, memo: form.memo }
        : null,
    errors,
  };
}

/** The pool's reasons, in words. */
const REASONS: Record<string, string> = {
  FeePayerAccountNotFound:
    "The sending account doesn't exist on devnet yet: fund it from the faucet first.",
  InsufficientFunds: "The balance doesn't cover the amount and the fee.",
  InvalidNonce:
    "The nonce isn't the account's next one: another transaction from this key may be pending.",
  Duplicate: "The pool already has this transaction.",
  InsufficientReplaceFee:
    "A transaction with this nonce is already pending, with a fee this one doesn't beat.",
  Expired: "It expired before the pool took it.",
  Overloaded: "The pool is full: raise the fee, or try later.",
  FeePayerNotPermittedToSend: "This account's permissions don't let it send.",
};

export function rejectionText(reason: string): string {
  return reason
    .split("; ")
    .map(r => REASONS[r.replace(/^"|"$/g, "")] ?? r)
    .join(" ");
}

export type SentStage =
  | "pending" // in the pool
  | "included" // in a best-chain block
  | "confirmed" // CONFIRMATIONS blocks on top
  | "failed" // included as failed
  | "dropped" // left the pool unseen in the best chain
  | "rejected"; // the pool refused it

export interface Sent {
  hash: string;
  /** The sender: nonces and tracking are per sender. */
  from: string;
  to: string;
  amount: bigint;
  fee: bigint;
  nonce: number;
  memo: string;
  sentAt: number;
  stage: SentStage;
  detail: string | null;
  inclusion: Inclusion | null;
  /** Absent: this run's node has had it, in its pool or a block. Set for a
   * payment kept from before a restart, whose node starts with an empty
   * pool: "unchecked" until a look answers, "failed" while looks fail,
   * then "missing" while a look finds it in neither the pool nor a
   * block. */
  check?: "unchecked" | "failed" | "missing";
}

/** One look at the node for a sent payment. `undefined` is a lookup that
 * failed: unknown, not absent. */
export interface Look {
  inclusion: InclusionLook | null | undefined;
  inPool: boolean | undefined;
  accountNonce: number | null | undefined;
}

/**
 * The next stage of a sent payment, from one look at the node: where it is
 * in the best chain, whether the pool still has it, and the sender's nonce
 * at the best tip. A lookup that failed changes nothing it would decide.
 */
export function nextStage(
  sent: Sent,
  look: Look,
): Pick<Sent, "stage" | "detail" | "inclusion"> {
  const keep = {
    stage: sent.stage,
    detail: sent.detail,
    inclusion: sent.inclusion,
  };
  if (sent.stage === "rejected" || sent.stage === "failed") return keep;
  const l = look.inclusion;
  // No answer, or no best tip yet: nothing to go on.
  if (l === undefined || l === null) return keep;
  if (l.found) {
    if (l.found.failure) {
      return {
        stage: "failed",
        // The ledger's failure name in words: "Amount_insufficient_to_
        // create_account" reads "amount insufficient to create account".
        detail: `Included as failed: ${l.found.failure.replace(/_/g, " ").toLowerCase()}.`,
        inclusion: l.found,
      };
    }
    return {
      stage: l.found.depth >= CONFIRMATIONS ? "confirmed" : "included",
      detail: null,
      inclusion: l.found,
    };
  }
  if (sent.inclusion) {
    // Seen in a block that is now older than the search reaches: it aged
    // out of the window, so it stays where it was.
    const oldest = l.tipHeight - l.depthSearched + 1;
    if (sent.inclusion.height < oldest) return keep;
    // Its block is within the window but no longer in the best chain: a
    // reorg took it out. It goes back to waiting, below.
  }
  const left = sent.inclusion
    ? `Its block ${sent.inclusion.height} left the best chain. `
    : "";
  if (look.inPool === undefined || look.accountNonce === undefined) {
    // Out of the chain, and the rest unknown: say so, and look again.
    return sent.inclusion
      ? {
          stage: "pending",
          detail: `${left}Looking for it again.`,
          inclusion: null,
        }
      : keep;
  }
  if (look.inPool) {
    return {
      stage: "pending",
      detail: left ? `${left}It is back in the pool.` : null,
      inclusion: null,
    };
  }
  if (look.accountNonce !== null && look.accountNonce > sent.nonce) {
    return {
      stage: "dropped",
      detail: `${left}Its nonce is used; the page didn't see it in a block.`,
      inclusion: null,
    };
  }
  return {
    stage: "pending",
    detail: `${left}Not in this node's pool now; waiting for a block.`,
    inclusion: null,
  };
}
