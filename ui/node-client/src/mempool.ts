/** Pending transactions: `GET /transaction-pool`, as the Angular mempool
 * page reads it. Fees come as MINA decimals, payment amounts as nanomina. */

import { minaToNanomina } from "./format";
import { arr, num, obj, str } from "./json";
import { decodeMemo } from "./memo";

export interface PendingTransaction {
  hash: string | null;
  kind: "Payment" | "Delegation" | "zkApp transaction";
  sender: string | null;
  receiver: string | null;
  /** Nanomina. */
  amount: bigint | null;
  fee: bigint | null;
  nonce: number | null;
  memo: string | null;
}

export function parsePendingTransaction(
  value: unknown,
): PendingTransaction | null {
  const t = obj(value);
  const data = arr(t?.data);
  const tag = str(data[0]);
  const command = obj(data[1]);
  if (!t || !tag || !command) return null;
  const hash = str(t.hash);
  if (tag === "Signed_command") {
    const payload = obj(command.payload) ?? {};
    const common = obj(payload.common) ?? {};
    const body = arr(payload.body);
    const bodyKind = str(body[0]);
    // A delegation's body is ["Stake_delegation", ["Set_delegate", {...}]].
    const details = obj(body[1]) ?? obj(arr(body[1])[1]) ?? {};
    const fee = str(common.fee);
    const rawAmount = str(details.amount);
    return {
      hash,
      kind: bodyKind === "Stake_delegation" ? "Delegation" : "Payment",
      sender: str(common.fee_payer_pk),
      receiver: str(details.receiver_pk) ?? str(details.new_delegate),
      amount:
        rawAmount !== null && /^\d+$/.test(rawAmount)
          ? BigInt(rawAmount)
          : null,
      fee: fee !== null ? minaToNanomina(fee) : null,
      nonce: num(common.nonce),
      memo: decodeMemo(str(common.memo)),
    };
  }
  if (tag === "Zkapp_command") {
    const feePayer = obj(obj(command.fee_payer)?.body) ?? {};
    const fee = str(feePayer.fee);
    return {
      hash,
      kind: "zkApp transaction",
      sender: str(feePayer.public_key),
      receiver: null,
      amount: null,
      fee: fee !== null ? minaToNanomina(fee) : null,
      nonce: num(feePayer.nonce),
      memo: decodeMemo(str(command.memo)),
    };
  }
  return null;
}

/** Highest fee first, as a block producer would take them. */
export function parseTransactionPool(raw: unknown): PendingTransaction[] {
  return arr(raw)
    .map(parsePendingTransaction)
    .filter((t): t is PendingTransaction => t !== null)
    .sort((a, b) => Number((b.fee ?? 0n) - (a.fee ?? 0n)));
}
