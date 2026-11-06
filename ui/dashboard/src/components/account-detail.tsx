/** An account's balance, nonce and delegate, and its vesting schedule if
 * it has one. */

import { Amount, Measure } from "@minanode/design-system/patterns/amount";
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
  // Alone at full width, values start a label column in.
  const hashEnd = timed ? "sm:justify-end" : undefined;
  return (
    // Two columns only with the vesting list beside the account.
    <div className={timed ? "grid items-start gap-6 lg:grid-cols-2" : ""}>
      <KeyValueList
        title="Account"
        align={timed ? "right" : "left"}
        rows={[
          {
            label: "Public key",
            value: (
              <Hash
                value={a.publicKey}
                label="public key"
                className={hashEnd}
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
            prose: a.delegate === null || a.delegate === a.publicKey,
            value:
              a.delegate === null ? (
                "Nobody"
              ) : a.delegate === a.publicKey ? (
                "Itself"
              ) : (
                <Hash
                  value={a.delegate}
                  label="delegate key"
                  className={hashEnd}
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
              value: (
                <Measure
                  value={`${formatCount(a.timing.vestingPeriod)} ${a.timing.vestingPeriod === 1 ? "slot" : "slots"}`}
                />
              ),
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
