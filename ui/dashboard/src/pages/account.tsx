import * as React from "react";
import { Search, Wallet } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { Input } from "@minanode/design-system/components/input";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PUBLIC_KEY_PATTERN } from "@minanode/node-client";
import { useAccount } from "@minanode/node-client/react";
import { AccountDetail } from "../components/account-detail";
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
        aside={
          valid ? <PolledAge polled={account} staleAfterMs={120_000} /> : null
        }
      />
      <AccountSearch
        initial={publicKey ?? ""}
        initialInvalid={publicKey !== null && !valid}
      />
      {publicKey === null || !valid ? null : (
        <Loaded polled={account} what="this account" rows={5}>
          {a =>
            a === null ? (
              <EmptyState
                icon={<Wallet />}
                title="No account for this key"
                description={
                  <p>
                    The ledger at the best tip has none. An account is created
                    when it first receives MINA, for a fee of 1 MINA.
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

const KEY_SHAPE = "A public key starts with B62 and has 55 characters.";

function AccountSearch({
  initial,
  initialInvalid,
}: {
  initial: string;
  /** The address's key isn't one: the field says so, as on a submit. */
  initialInvalid: boolean;
}) {
  const [value, setValue] = React.useState(initial);
  const [error, setError] = React.useState<string | null>(
    initialInvalid ? KEY_SHAPE : null,
  );
  const id = React.useId();
  const errorId = React.useId();
  React.useEffect(() => {
    setValue(initial);
    setError(initialInvalid ? KEY_SHAPE : null);
  }, [initial, initialInvalid]);
  return (
    <form
      role="search"
      noValidate
      className="flex flex-col gap-1.5 rounded-xl border bg-card p-4"
      onSubmit={event => {
        event.preventDefault();
        const key = value.trim();
        if (!PUBLIC_KEY_PATTERN.test(key)) {
          setError(KEY_SHAPE);
          return;
        }
        setError(null);
        navigate(`/account/${key}`);
      }}
    >
      <label htmlFor={id} className="text-sm font-medium text-foreground">
        Public key
      </label>
      <div className="flex gap-2">
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
        <Button type="submit">
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
