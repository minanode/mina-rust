import * as React from "react";
import { ArrowDown, Search } from "lucide-react";

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
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import {
  blockCounts,
  blockQuery,
  commandFees,
  formatCount,
  formatHeight,
  formatMina,
  plural,
  RECENT_BLOCKS_LENGTH,
  type Block,
} from "@minanode/node-client";
import { cn } from "@minanode/design-system/lib/utils";
import { useBestChain } from "@minanode/node-client/react";
import { Count, Mina, TABLE_LINK } from "../components/cells";
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
        aside={<PolledAge polled={blocks} staleAfterMs={POLL_MS * 4} />}
      />
      <BlockSearch />
      <Loaded polled={blocks} what="the best chain" rows={8}>
        {list => (
          <>
            <BlockList list={list} />
            <Table
              className="[&_td]:align-middle [&_td]:whitespace-nowrap"
              containerProps={{
                ...tableRegion("Best chain blocks"),
                className: "hidden sm:block",
              }}
            >
              <TableHeader>
                <TableRow>
                  <TableHead aria-sort="descending">
                    <span className="inline-flex items-center gap-1">
                      Height
                      <ArrowDown aria-hidden className="size-3" />
                    </span>
                  </TableHead>
                  <TableHead>When</TableHead>
                  <TableHead className="text-right">Slot</TableHead>
                  <TableHead>Producer</TableHead>
                  <TableHead className="text-right">Transactions</TableHead>
                  <TableHead className="text-right">SNARK work</TableHead>
                  <TableHead className="text-right">Fees (MINA)</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {list.map(b => {
                  const counts = blockCounts(b);
                  const fees = commandFees(b);
                  return (
                    <TableRow key={b.stateHash}>
                      <TableCell className="font-mono tabular-nums">
                        <Link
                          to={`/blocks/${b.stateHash}`}
                          className={TABLE_LINK}
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
                        <Count
                          n={
                            counts.payments + counts.delegations + counts.zkapps
                          }
                        />
                        {counts.zkapps ? (
                          <span className="text-muted-foreground">
                            {" "}
                            ({counts.zkapps} zkApp)
                          </span>
                        ) : null}
                      </TableCell>
                      <TableCell className="text-right font-mono tabular-nums">
                        <Count n={counts.works} />
                      </TableCell>
                      <TableCell className="text-right">
                        <Mina nanomina={fees} />
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          </>
        )}
      </Loaded>
    </>
  );
}

/** The blocks on a phone: a row each, its figures in columns that line up
 * row to row. */
function BlockList({ list }: { list: Block[] }) {
  return (
    <ul
      aria-label="Best chain blocks"
      className="flex flex-col divide-y rounded-xl border bg-card sm:hidden"
    >
      {list.map(b => {
        const counts = blockCounts(b);
        const txs = counts.payments + counts.delegations + counts.zkapps;
        const fees = commandFees(b);
        return (
          <li key={b.stateHash} className="flex flex-col gap-1 px-4 py-3">
            <div className="flex items-center justify-between gap-3 text-sm">
              <Link
                to={`/blocks/${b.stateHash}`}
                className={`font-mono tabular-nums ${TABLE_LINK}`}
              >
                {formatHeight(b.height)}
              </Link>
              {b.creator ? <Hash value={b.creator} copy={false} /> : null}
            </div>
            {/* When and how many on the left; the fees, one unit, right. */}
            <p className="flex items-baseline justify-between gap-3 text-xs text-muted-foreground">
              <span>
                <TimeAgo at={b.timeMs} />
                {" · "}
                <span className={txs ? "text-foreground" : undefined}>
                  {plural(txs, "transaction")}
                </span>
              </span>
              {fees !== null ? (
                <span className="whitespace-nowrap">
                  fees{" "}
                  <span
                    className={cn(
                      "font-mono tabular-nums",
                      fees !== 0n && "text-foreground",
                    )}
                  >
                    {formatMina(fees)}
                  </span>{" "}
                  MINA
                </span>
              ) : null}
            </p>
          </li>
        );
      })}
    </ul>
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
      className="flex flex-col gap-1.5 rounded-xl border bg-card p-4"
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
        Block height or state hash
      </label>
      <div className="flex gap-2">
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
