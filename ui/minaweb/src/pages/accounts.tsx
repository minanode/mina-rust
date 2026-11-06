import * as React from "react";
import { Search, Wallet } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { Input } from "@minanode/design-system/components/input";
import {
  tableRegion,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@minanode/design-system/components/table";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import {
  formatHeight,
  MINA_TOKEN_ID,
  PUBLIC_KEY_PATTERN,
  shortHash,
  type LedgerAccount,
} from "@minanode/node-client";
import {
  useLedgerAccounts,
  useNodeStatus,
  type Polled,
} from "@minanode/node-client/react";
import { AccountDetail } from "minanode-dashboard/components/account-detail";
import { Loaded, PolledAge } from "minanode-dashboard/components/states";

import { navigate } from "../app/router";

const POLL_MS = 30_000;

function AccountSearch({
  initial,
  initialInvalid,
}: {
  initial: string;
  /** The address's key isn't one: the field says so, as on a submit. */
  initialInvalid: boolean;
}) {
  const [value, setValue] = React.useState(initial);
  const [invalid, setInvalid] = React.useState(initialInvalid);
  const id = React.useId();
  const errorId = React.useId();
  React.useEffect(() => {
    setValue(initial);
    setInvalid(initialInvalid);
  }, [initial, initialInvalid]);
  return (
    <form
      role="search"
      className="flex flex-col gap-1.5 rounded-xl border bg-card p-4"
      onSubmit={event => {
        event.preventDefault();
        const key = value.trim();
        if (!PUBLIC_KEY_PATTERN.test(key)) {
          setInvalid(true);
          return;
        }
        navigate(`/accounts/${key}`);
      }}
    >
      <label htmlFor={id} className="text-sm font-medium text-foreground">
        Public key
      </label>
      <div className="flex gap-2">
        <Input
          id={id}
          value={value}
          placeholder="B62…"
          spellCheck={false}
          autoComplete="off"
          aria-invalid={invalid || undefined}
          aria-describedby={invalid ? errorId : undefined}
          onChange={event => {
            setValue(event.target.value);
            setInvalid(false);
          }}
          className="font-mono"
        />
        <Button type="submit">
          <Search aria-hidden />
          Look up
        </Button>
      </div>
      {invalid ? (
        <p id={errorId} className="text-sm text-destructive">
          A Mina public key starts with B62 and has 55 characters.
        </p>
      ) : null}
    </form>
  );
}

function Zkapp({ account: a }: { account: LedgerAccount }) {
  if (!a.zkapp) return null;
  const z = a.zkapp;
  return (
    <KeyValueList
      title="zkApp"
      rows={[
        {
          label: "Verification key hash",
          value: z.verificationKeyHash
            ? shortHash(z.verificationKeyHash, 10, 6)
            : "None",
        },
        { label: "Proved state", value: z.provedState ? "Yes" : "No" },
        { label: "zkApp version", value: z.zkappVersion ?? "–" },
        { label: "URI", value: z.zkappUri ?? "–" },
        ...z.appState.map((field, i) => ({
          label: `State ${i}`,
          value: (
            <span className="font-mono text-xs break-all" title={field}>
              {field.length > 24 ? shortHash(field, 12, 8) : field}
            </span>
          ),
        })),
      ]}
    />
  );
}

function Tokens({ accounts }: { accounts: LedgerAccount[] }) {
  const others = accounts.filter(a => a.tokenId !== MINA_TOKEN_ID);
  if (others.length === 0) return null;
  return (
    <PageSection
      title="Other tokens"
      description="Accounts this key holds for tokens other than MINA."
    >
      <Table containerProps={tableRegion("Token accounts")}>
        <TableHeader>
          <TableRow>
            <TableHead>Token id</TableHead>
            <TableHead>Symbol</TableHead>
            <TableHead className="text-right">Balance</TableHead>
            <TableHead className="text-right">Nonce</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {others.map(a => (
            <TableRow key={a.tokenId ?? ""}>
              <TableCell className="font-mono text-xs">
                {a.tokenId ? shortHash(a.tokenId, 8, 6) : "–"}
              </TableCell>
              <TableCell>{a.tokenSymbol ?? "–"}</TableCell>
              <TableCell className="text-right font-mono tabular-nums">
                {a.balance ?? "–"}
              </TableCell>
              <TableCell className="text-right font-mono tabular-nums">
                {a.nonce ?? "–"}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </PageSection>
  );
}

function Lookup({
  accounts,
  tip,
}: {
  accounts: Polled<LedgerAccount[] | null>;
  tip: { height: number } | null;
}) {
  if (!tip) {
    return (
      <EmptyState
        icon={<Wallet />}
        title="The node has no ledger yet"
        description={
          <p>
            Accounts can be looked up once the node has synced to a best tip.
          </p>
        }
      />
    );
  }
  return (
    <Loaded polled={accounts} what="this account" rows={5}>
      {list => {
        if (list === null) {
          return (
            <EmptyState
              icon={<Wallet />}
              title="No answer from the ledger"
              description={<p>The node gave no ledger to read from.</p>}
            />
          );
        }
        const mina = list.find(a => a.tokenId === MINA_TOKEN_ID) ?? null;
        if (list.length === 0) {
          return (
            <EmptyState
              icon={<Wallet />}
              title="Not in this node's ledger"
              description={
                <p>
                  The ledger at the best tip ({formatHeight(tip.height)}) has no
                  account for this key. An account is created when it first
                  receives MINA, for a fee of 1 MINA.
                </p>
              }
            />
          );
        }
        return (
          <>
            {mina ? (
              <div
                data-account={mina.publicKey}
                data-balance={mina.balance ?? ""}
              >
                <AccountDetail
                  account={{ ...mina, balanceHeight: tip.height }}
                />
              </div>
            ) : (
              <EmptyState
                icon={<Wallet />}
                title="No MINA account"
                description={
                  <p>The key holds only other tokens, listed below.</p>
                }
              />
            )}
            {mina ? <Zkapp account={mina} /> : null}
            <Tokens accounts={list} />
          </>
        );
      }}
    </Loaded>
  );
}

export function AccountsPage({ publicKey }: { publicKey: string | null }) {
  const valid = publicKey !== null && PUBLIC_KEY_PATTERN.test(publicKey);
  const { status } = useNodeStatus();
  const tip = status.data?.bestTip ?? null;
  // The ledger read waits for a best tip: without one, don't ask.
  const accounts = useLedgerAccounts(valid && tip ? publicKey : null, POLL_MS);
  return (
    <>
      <PageHeader
        eyebrow="Chain"
        title="Accounts"
        aside={
          valid && tip ? (
            <PolledAge polled={accounts} staleAfterMs={POLL_MS * 4} />
          ) : null
        }
      />
      <AccountSearch
        initial={publicKey ?? ""}
        initialInvalid={publicKey !== null && !valid}
      />
      {publicKey === null || !valid ? null : (
        <Lookup accounts={accounts} tip={tip} />
      )}
    </>
  );
}
