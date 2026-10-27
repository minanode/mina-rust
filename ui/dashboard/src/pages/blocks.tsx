import * as React from "react";
import { Search } from "lucide-react";

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
import { Hash } from "@minanode/design-system/patterns/hash";
import { Amount } from "@minanode/design-system/patterns/amount";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import {
  blockCounts,
  blockQuery,
  commandFees,
  formatCount,
  formatHeight,
  formatMina,
  RECENT_BLOCKS_LENGTH,
} from "@minanode/node-client";
import { useBestChain } from "@minanode/node-client/react";
import { Loaded, PolledAge } from "../components/states";

import { Link, navigate } from "../app/router";

const POLL_MS = 10_000;

export function BlocksPage() {
  const blocks = useBestChain(RECENT_BLOCKS_LENGTH, POLL_MS);

  return (
    <>
      <PageHeader
        eyebrow="Chain"
        title="Blocks"
        description={`The newest ${RECENT_BLOCKS_LENGTH} blocks of this node's best chain, newest first.`}
        aside={<PolledAge polled={blocks} staleAfterMs={POLL_MS * 4} />}
      />
      <BlockSearch />
      <Loaded polled={blocks} what="the best chain" rows={8}>
        {list => (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={tableRegion("Best chain blocks")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Height</TableHead>
                <TableHead>When</TableHead>
                <TableHead className="text-right">Slot</TableHead>
                <TableHead>Producer</TableHead>
                <TableHead className="text-right">Transactions</TableHead>
                <TableHead className="text-right">SNARK work</TableHead>
                <TableHead className="text-right">Fees</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {list.map(b => {
                const counts = blockCounts(b);
                const fees = commandFees(b);
                return (
                  <TableRow key={b.stateHash}>
                    <TableCell className="font-mono">
                      <Link
                        to={`/blocks/${b.stateHash}`}
                        className="rounded-sm text-brand-text underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring"
                      >
                        {formatHeight(b.height)}
                      </Link>
                    </TableCell>
                    <TableCell>
                      <TimeAgo at={b.timeMs} />
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      {formatCount(b.slot)}
                    </TableCell>
                    <TableCell>
                      {b.creator ? (
                        <Hash value={b.creator} copy={false} />
                      ) : (
                        "–"
                      )}
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      {formatCount(
                        counts.payments + counts.delegations + counts.zkapps,
                      )}
                      {counts.zkapps ? (
                        <span className="text-muted-foreground">
                          {" "}
                          ({counts.zkapps} zkApp)
                        </span>
                      ) : null}
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      {formatCount(counts.works)}
                    </TableCell>
                    <TableCell className="text-right">
                      <Amount
                        unit="MINA"
                        value={fees === null ? null : formatMina(fees)}
                      />
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </Loaded>
    </>
  );
}

function BlockSearch() {
  const [value, setValue] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);
  const id = React.useId();
  const errorId = React.useId();
  return (
    <form
      role="search"
      noValidate
      className="flex flex-col gap-2"
      onSubmit={event => {
        event.preventDefault();
        if (!blockQuery(value)) {
          setError(
            "Enter a block height, like 578883, or a state hash starting with 3N.",
          );
          return;
        }
        setError(null);
        navigate(`/blocks/${encodeURIComponent(value.trim())}`);
      }}
    >
      <label htmlFor={id} className="text-sm font-medium text-foreground">
        Find a block by height or state hash
      </label>
      <div className="flex max-w-2xl gap-2">
        <Input
          id={id}
          value={value}
          onChange={e => setValue(e.target.value)}
          placeholder="578883 or 3N…"
          autoComplete="off"
          spellCheck={false}
          className="font-mono"
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : undefined}
        />
        <Button type="submit" variant="secondary">
          <Search aria-hidden />
          Find
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
