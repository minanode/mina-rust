import * as React from "react";
import { Search, Wallet } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { Input } from "@minanode/design-system/components/input";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { Amount } from "@minanode/design-system/patterns/amount";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import {
  formatCount,
  formatHeight,
  formatMina,
  isTimed,
  PUBLIC_KEY_PATTERN,
  type Account,
} from "@minanode/node-client";
import { useAccount } from "@minanode/node-client/react";
import { Loaded, PolledAge } from "../components/states";

import { navigate } from "../app/router";

export function AccountPage({ publicKey }: { publicKey: string | null }) {
  const valid = publicKey !== null && PUBLIC_KEY_PATTERN.test(publicKey);
  const account = useAccount(valid ? publicKey : null);

  return (
    <>
      <PageHeader
        eyebrow="Chain"
        title="Accounts"
        description="Look up an account in this node's ledger, at its best tip."
        aside={
          valid ? <PolledAge polled={account} staleAfterMs={120_000} /> : null
        }
      />
      <AccountSearch initial={publicKey ?? ""} />
      {publicKey === null ? (
        <EmptyState
          icon={<Wallet />}
          title="Enter a public key"
          description={
            <p>A Mina public key starts with B62 and has 55 characters.</p>
          }
        />
      ) : !valid ? (
        <EmptyState
          tone="caution"
          title="That isn't a public key"
          description={
            <p>A Mina public key starts with B62 and has 55 characters.</p>
          }
        />
      ) : (
        <Loaded polled={account} what="this account" rows={5}>
          {a =>
            a === null ? (
              <EmptyState
                icon={<Wallet />}
                title="No account for this key"
                description={
                  <p>
                    The ledger at the best tip has no account for it. An account
                    is created when it first receives MINA, for a fee of 1 MINA.
                  </p>
                }
              />
            ) : (
              <AccountDetail account={a} />
            )
          }
        </Loaded>
      )}
    </>
  );
}

function AccountSearch({ initial }: { initial: string }) {
  const [value, setValue] = React.useState(initial);
  const [error, setError] = React.useState<string | null>(null);
  const id = React.useId();
  const errorId = React.useId();
  React.useEffect(() => setValue(initial), [initial]);
  return (
    <form
      role="search"
      noValidate
      className="flex flex-col gap-2"
      onSubmit={event => {
        event.preventDefault();
        const key = value.trim();
        if (!PUBLIC_KEY_PATTERN.test(key)) {
          setError("A public key starts with B62 and has 55 characters.");
          return;
        }
        setError(null);
        navigate(`/account/${key}`);
      }}
    >
      <label htmlFor={id} className="text-sm font-medium text-foreground">
        Public key
      </label>
      <div className="flex max-w-2xl gap-2">
        <Input
          id={id}
          value={value}
          onChange={e => setValue(e.target.value)}
          placeholder="B62q…"
          autoComplete="off"
          spellCheck={false}
          className="font-mono"
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : undefined}
        />
        <Button type="submit" variant="secondary">
          <Search aria-hidden />
          Look up
        </Button>
      </div>
      {error ? (
        <p id={errorId} className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </form>
  );
}

function AccountDetail({ account: a }: { account: Account }) {
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
