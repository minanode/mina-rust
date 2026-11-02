/** An account's balance, nonce and delegate, and its vesting schedule if
 * it has one. */

import { Amount } from "@minanode/design-system/patterns/amount";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import {
  formatCount,
  formatHeight,
  formatMina,
  isTimed,
  type Account,
} from "@minanode/node-client";

export function AccountDetail({ account: a }: { account: Account }) {
  const timed = isTimed(a);
  return (
    <div className="grid gap-6 lg:grid-cols-2">
      <KeyValueList
        title="Account"
        rows={[
          {
            label: "Public key",
            value: (
              <Hash
                value={a.publicKey}
                label="public key"
                className="sm:justify-end"
              />
            ),
          },
          {
            label: "Balance",
            value: <Amount unit="MINA" value={formatMina(a.balance)} />,
            tone: "brand",
          },
          ...(timed
            ? [
                {
                  label: "Liquid",
                  value: <Amount unit="MINA" value={formatMina(a.liquid)} />,
                },
                {
                  label: "Locked",
                  value: <Amount unit="MINA" value={formatMina(a.locked)} />,
                },
              ]
            : []),
          { label: "Nonce", value: a.nonce ?? "–" },
          {
            label: "Delegates to",
            value:
              a.delegate === null ? (
                "Nobody"
              ) : a.delegate === a.publicKey ? (
                "Itself"
              ) : (
                <Hash
                  value={a.delegate}
                  label="delegate key"
                  className="sm:justify-end"
                />
              ),
          },
          {
            label: "Balance as of block",
            value: formatHeight(a.balanceHeight),
          },
        ]}
      />
      {timed ? (
        <KeyValueList
          title="Vesting"
          rows={[
            {
              label: "Initial minimum balance",
              value: (
                <Amount
                  unit="MINA"
                  value={formatMina(a.timing.initialMinimumBalance)}
                />
              ),
            },
            { label: "Cliff slot", value: formatCount(a.timing.cliffTime) },
            {
              label: "Cliff amount",
              value: (
                <Amount unit="MINA" value={formatMina(a.timing.cliffAmount)} />
              ),
            },
            {
              label: "Vesting period",
              value: `${formatCount(a.timing.vestingPeriod)} slots`,
            },
            {
              label: "Vesting increment",
              value: (
                <Amount
                  unit="MINA"
                  value={formatMina(a.timing.vestingIncrement)}
                />
              ),
            },
          ]}
        />
      ) : null}
    </div>
  );
}
