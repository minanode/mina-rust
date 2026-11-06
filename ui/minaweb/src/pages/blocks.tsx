import * as React from "react";
import { ArrowDown, Blocks, Search } from "lucide-react";

import { Badge } from "@minanode/design-system/components/badge";
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
import { Measure } from "@minanode/design-system/patterns/amount";
import { Hash } from "@minanode/design-system/patterns/hash";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import { Signal, type Tone } from "@minanode/design-system/patterns/signal";
import {
  blockQuery,
  formatCount,
  formatHeight,
  formatMina,
  plural,
  receiptOutcome,
  type BlockHeader,
  type Check,
  type Receipt,
  type NodeClient,
  type Verification,
  VERIFICATION_MAX_PAGE,
} from "@minanode/node-client";
import {
  useBestChainHeaders,
  useNodeClient,
  useNodeStatus,
  usePoll,
  useVerificationOf,
} from "@minanode/node-client/react";
import { useShowAll } from "minanode-dashboard/components/show-all";
import { Loaded, PolledAge } from "minanode-dashboard/components/states";

import { cn } from "@minanode/design-system/lib/utils";

import { Link, navigate } from "../app/router";
import { useStatusCatchUp } from "../lib/tip";

const POLL_MS = 5_000;
const SHOWN = 40;
/** Best-chain blocks a page lists. */
export const CHAIN_PAGE = 30;
/** Pages of receipts read: a bootstrap records about 290 blocks after the
 * tips it saw first, which three pages still reach. */
const PAGES = 3;

/** Up to PAGES pages of receipts, in the node's order (newest first). */
export async function recentReceipts(
  client: Pick<NodeClient, "verification">,
  signal?: AbortSignal,
): Promise<Verification> {
  const first = await client.verification(
    { limit: VERIFICATION_MAX_PAGE },
    signal,
  );
  const receipts = [...first.receipts];
  let page = first.receipts;
  for (let i = 1; i < PAGES && page.length === VERIFICATION_MAX_PAGE; i++) {
    page = (
      await client.verification(
        { limit: VERIFICATION_MAX_PAGE, offset: i * VERIFICATION_MAX_PAGE },
        signal,
      )
    ).receipts;
    receipts.push(...page);
  }
  return { ...first, receipts };
}

/** The highest receipts, with the last block whose proof passed, which an
 * older receipt may have pushed past the pages read. */
export function shownReceipts(v: Verification, shown = SHOWN): Receipt[] {
  const last = v.lastVerified;
  const all =
    last && !v.receipts.some(r => r.stateHash === last.stateHash)
      ? [last, ...v.receipts]
      : v.receipts;
  const seen = new Set<string>();
  return all
    .filter(r => !seen.has(r.stateHash) && seen.add(r.stateHash))
    .sort(byHeight)
    .slice(0, shown);
}

/** Highest first; the receipts come in the order the node got them. */
function byHeight(a: Receipt, b: Receipt) {
  return (
    (b.height ?? -1) - (a.height ?? -1) ||
    (b.receivedAtMs ?? 0) - (a.receivedAtMs ?? 0)
  );
}

export const ARRIVAL: Record<string, string> = {
  gossip: "Gossip",
  catchup: "Catch-up",
  bootstrap: "Bootstrap",
  bestTip: "Best tip",
  local: "Produced here",
};

/** The docs section on blocks the node did not check. */
export const NOT_CHECKED_DOCS_URL =
  "https://minanode.org/docs/web-node-connections/#not-checked";

export const OUTCOME: Record<
  ReturnType<typeof receiptOutcome>,
  { tone: Tone; label: string }
> = {
  passed: { tone: "ok", label: "Passed" },
  failed: { tone: "error", label: "Failed" },
  pending: { tone: "info", label: "Checking" },
  unchecked: { tone: "neutral", label: "Not checked" },
};

function proofTime(check: Check): string | null {
  return check.status === "passed" && check.elapsedMs !== null
    ? `${Math.round(check.elapsedMs)} ms`
    : null;
}

export function ProofCell({
  receipt,
  time: showTime = true,
}: {
  receipt: Receipt;
  /** The check's duration beside it, when it passed. */
  time?: boolean;
}) {
  const { tone, label } = OUTCOME[receiptOutcome(receipt)];
  const time = showTime ? proofTime(receipt.proof) : null;
  return (
    <span className="flex items-center gap-2">
      {/* On every row: the dot and the word, without a pill. */}
      <Signal tone={tone} variant="plain">
        {label}
      </Signal>
      {time ? <Measure className="text-muted-foreground" value={time} /> : null}
    </span>
  );
}

/** A column head the rows are sorted by, highest first. */
function SortedHead({ children }: { children: React.ReactNode }) {
  return (
    <TableHead aria-sort="descending">
      <span className="inline-flex items-center gap-1">
        {children}
        <ArrowDown aria-hidden className="size-3" />
      </span>
    </TableHead>
  );
}

/** A count, muted when it is zero so the others stand out. */
function Count({ n }: { n: number }) {
  return (
    <span className={n === 0 ? "text-muted-foreground" : undefined}>
      {formatCount(n)}
    </span>
  );
}

/** A height or hash that opens its page: in the text colour, as a table
 * of them would be a column of teal otherwise; teal and underlined on
 * hover. */
export const linkClass =
  "rounded-sm text-foreground underline decoration-border underline-offset-4 hover:text-brand-text hover:decoration-current focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring";

/** The fees a header lists, commands' and SNARK work's, in MINA: the
 * column's head names the unit. */
function Fees({ h }: { h: BlockHeader }) {
  if (h.transactionFees === null) {
    return <span className="text-muted-foreground">–</span>;
  }
  const fees = formatMina(h.transactionFees);
  return (
    <span
      className={cn(
        "font-mono tabular-nums",
        h.transactionFees === 0n && "text-muted-foreground",
      )}
    >
      {fees}
    </span>
  );
}

function BlockSearch() {
  const [value, setValue] = React.useState("");
  const [invalid, setInvalid] = React.useState(false);
  const id = React.useId();
  const errorId = React.useId();
  return (
    <form
      role="search"
      className="flex max-w-3xl flex-col gap-1.5 rounded-xl border bg-card p-4"
      onSubmit={event => {
        event.preventDefault();
        const trimmed = value.trim();
        if (blockQuery(trimmed) === null) {
          setInvalid(true);
          return;
        }
        navigate(`/blocks/${encodeURIComponent(trimmed)}`);
      }}
    >
      <label htmlFor={id} className="text-sm font-medium text-foreground">
        Height or state hash
      </label>
      <div className="flex gap-2">
        <Input
          id={id}
          value={value}
          placeholder="585170 or 3N…"
          aria-invalid={invalid || undefined}
          aria-describedby={invalid ? errorId : undefined}
          onChange={event => {
            setValue(event.target.value);
            setInvalid(false);
          }}
          className="font-mono"
        />
        <Button type="submit" variant="outline">
          <Search aria-hidden />
          Look up
        </Button>
      </div>
      {invalid ? (
        <p id={errorId} className="text-sm text-destructive">
          A height is a number; a state hash starts with 3N and has 52
          characters.
        </p>
      ) : null}
    </form>
  );
}

/** Below 640 px the best chain is a list, where a table shows two of its
 * columns: height and proof check, then time, transactions and fees. */
function ChainList({
  headers,
  receipts,
  tip,
}: {
  headers: readonly BlockHeader[];
  receipts: Map<string, Receipt>;
  tip: string | null;
}) {
  return (
    <ul
      aria-label="Best chain blocks"
      className="flex flex-col divide-y rounded-xl border bg-card sm:hidden"
    >
      {headers.map(h => {
        const receipt = receipts.get(h.stateHash);
        return (
          <li key={h.stateHash} className="flex flex-col gap-1 px-4 py-3">
            <div className="flex items-center justify-between gap-3">
              <span className="flex items-center gap-2 text-sm">
                <Link
                  to={`/blocks/${h.stateHash}`}
                  className={`font-mono tabular-nums ${linkClass}`}
                >
                  {formatHeight(h.height)}
                </Link>
                {h.stateHash === tip ? (
                  <Badge font="sans">best tip</Badge>
                ) : null}
              </span>
              {receipt ? (
                <ProofCell receipt={receipt} time={false} />
              ) : (
                <span className="text-xs text-muted-foreground">No record</span>
              )}
            </div>
            {/* Age, transactions and fees in columns, so they line up row
                to row. */}
            <p className="grid grid-cols-[7.25rem_minmax(0,1fr)_auto] gap-x-3 text-xs text-muted-foreground">
              <TimeAgo at={h.timeMs} />
              {/* Figures that aren't zero in the text colour, as in the
                  table. */}
              <span className={h.userCommands ? "text-foreground" : undefined}>
                {plural(h.userCommands, "transaction")}
              </span>
              {h.transactionFees !== null ? (
                <span className="text-right">
                  fees{" "}
                  {/* A fixed figure width keeps "fees" in place row to row. */}
                  <span
                    className={cn(
                      "ml-0.5 inline-block min-w-[4.5ch] text-right font-mono tabular-nums",
                      h.transactionFees !== 0n && "text-foreground",
                    )}
                  >
                    {formatMina(h.transactionFees)}
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

/** What the best-chain section shows: the list; no chain yet (the newest
 * page is empty); or an older page whose blocks the root has passed, which
 * polling can't bring back, so the visitor needs a way to the newest. */
export function chainPage(
  below: number | null,
  chain: { headers: unknown[] },
): "list" | "empty" | "expired" {
  if (chain.headers.length > 0) return "list";
  return below === null ? "empty" : "expired";
}

export function BestChain() {
  // Paging down by height: null is the newest page.
  const [below, setBelow] = React.useState<number | null>(null);
  const chain = useBestChainHeaders(
    below === null
      ? { limit: CHAIN_PAGE }
      : { limit: CHAIN_PAGE, maxHeight: below },
    below === null ? POLL_MS : 60_000,
  );
  const hashes = React.useMemo(
    () => chain.data?.headers.map(h => h.stateHash) ?? [],
    [chain.data],
  );
  const checks = useVerificationOf(hashes, POLL_MS * 2);
  const receipts = new Map(
    (checks.data?.receipts ?? []).map(r => [r.stateHash, r]),
  );
  const { status } = useNodeStatus();
  const tip = status.data?.bestTip?.hash ?? null;
  // The newest page's first block is the best tip this list knows.
  useStatusCatchUp(
    below === null ? (chain.data?.headers[0]?.height ?? null) : null,
  );
  // Ten rows first, so Blocks received stays in reach; the page's others
  // on request.
  const { shown, toggle } = useShowAll(chain.data?.headers ?? [], "newest");
  // Paging sits with the section's other controls, by its title.
  const c = chain.data;
  const oldest = c?.headers[c.headers.length - 1] ?? null;
  const paging =
    c && oldest ? (
      <>
        {below !== null ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-action="newest"
            onClick={() => setBelow(null)}
          >
            Newest
          </Button>
        ) : null}
        {c.rootHeight !== null && oldest.height > c.rootHeight ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-action="older"
            onClick={() => setBelow(oldest.height - 1)}
          >
            Older
          </Button>
        ) : null}
      </>
    ) : null;

  return (
    <PageSection
      title="Best chain"
      stackOnPhone
      aside={
        <div className="flex flex-wrap items-center justify-end gap-x-2 gap-y-2 sm:gap-x-3">
          {paging}
          {toggle}
          <PolledAge polled={chain} staleAfterMs={POLL_MS * 4} />
        </div>
      }
    >
      <Loaded polled={chain} what="the best chain" rows={8}>
        {c =>
          chainPage(below, c) === "expired" ? (
            <EmptyState
              icon={<Blocks />}
              title="These blocks have left the frontier"
              description={
                <p>
                  The node keeps the root and the blocks after it. The root has
                  moved past this page.
                </p>
              }
              data-chain-page="expired"
              action={
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  data-action="newest"
                  onClick={() => setBelow(null)}
                >
                  Newest
                </Button>
              }
            />
          ) : chainPage(below, c) === "empty" ? (
            <EmptyState
              icon={<Blocks />}
              title="No best chain yet"
              description={
                <p>The list fills once the node has synced its blocks.</p>
              }
              data-chain-page="empty"
            />
          ) : (
            <div className="flex flex-col gap-3">
              <ChainList headers={shown} receipts={receipts} tip={tip} />
              <Table
                density="compact"
                className="[&_td]:align-middle [&_td]:whitespace-nowrap"
                containerProps={{
                  ...tableRegion("Best chain blocks"),
                  className: "hidden sm:block",
                }}
              >
                <TableHeader>
                  <TableRow>
                    <SortedHead>Height</SortedHead>
                    <TableHead>When</TableHead>
                    <TableHead>Producer</TableHead>
                    <TableHead className="text-right">Transactions</TableHead>
                    <TableHead className="text-right">zkApp</TableHead>
                    <TableHead className="text-right">SNARK work</TableHead>
                    <TableHead className="text-right">Fees (MINA)</TableHead>
                    <TableHead>Proof</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {shown.map(h => {
                    const receipt = receipts.get(h.stateHash);
                    return (
                      <TableRow key={h.stateHash} data-height={h.height}>
                        <TableCell className="font-mono tabular-nums">
                          <Link
                            to={`/blocks/${h.stateHash}`}
                            className={linkClass}
                          >
                            {formatHeight(h.height)}
                          </Link>
                          {h.stateHash === tip ? (
                            <Badge font="sans" className="ml-2">
                              best tip
                            </Badge>
                          ) : null}
                          {h.height === c.rootHeight ? (
                            <Badge
                              variant="outline"
                              font="sans"
                              className="ml-2"
                            >
                              root
                            </Badge>
                          ) : null}
                        </TableCell>
                        <TableCell>
                          <TimeAgo at={h.timeMs} />
                        </TableCell>
                        <TableCell>
                          {h.producer ? (
                            <Hash value={h.producer} copy={false} />
                          ) : (
                            "–"
                          )}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums">
                          <Count n={h.userCommands} />
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums">
                          <Count n={h.zkappCommands} />
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums">
                          <Count n={h.snarkJobs} />
                        </TableCell>
                        <TableCell className="text-right">
                          <Fees h={h} />
                        </TableCell>
                        <TableCell>
                          {receipt ? (
                            <ProofCell receipt={receipt} />
                          ) : (
                            <span className="text-muted-foreground">
                              No record
                            </span>
                          )}
                        </TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </div>
          )
        }
      </Loaded>
    </PageSection>
  );
}

/** Every block the node received, highest first, with its checks: forks
 * and blocks it didn't apply too. */
function Received() {
  const client = useNodeClient();
  const checks = usePoll(
    `blocks:${client.baseUrl}`,
    signal => recentReceipts(client, signal),
    POLL_MS,
  );
  const { status } = useNodeStatus();
  const tip = status.data?.bestTip?.hash ?? null;
  const received = checks.data ? shownReceipts(checks.data) : [];
  const { shown, toggle } = useShowAll(received, "highest");

  return (
    <PageSection
      title="Blocks received"
      aside={
        <div className="flex items-center gap-2 sm:gap-3">
          {toggle}
          <PolledAge polled={checks} staleAfterMs={POLL_MS * 4} />
        </div>
      }
    >
      <Loaded polled={checks} what="the block checks" rows={8}>
        {v =>
          shownReceipts(v).length === 0 ? (
            <EmptyState
              icon={<Blocks />}
              title="No block received yet"
              description={
                <p>Blocks show here as the node receives and checks them.</p>
              }
            />
          ) : (
            <Table
              density="compact"
              className="[&_td]:align-middle [&_td]:whitespace-nowrap"
              containerProps={tableRegion("Blocks received")}
            >
              <TableHeader>
                <TableRow>
                  <SortedHead>Height</SortedHead>
                  <TableHead className="hidden md:table-cell">
                    State hash
                  </TableHead>
                  <TableHead className="hidden sm:table-cell">
                    Came by
                  </TableHead>
                  <TableHead className="hidden sm:table-cell">
                    Received
                  </TableHead>
                  <TableHead>Proof</TableHead>
                  <TableHead>Chain</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {shown.map(r => (
                  <TableRow key={r.stateHash}>
                    <TableCell className="font-mono tabular-nums">
                      <Link to={`/blocks/${r.stateHash}`} className={linkClass}>
                        {formatHeight(r.height)}
                      </Link>
                      {/* Beside the height, as in the best chain table. */}
                      {r.stateHash === tip ? (
                        <Badge font="sans" className="ml-2">
                          best tip
                        </Badge>
                      ) : null}
                    </TableCell>
                    <TableCell className="hidden md:table-cell">
                      <Hash value={r.stateHash} label="state hash" />
                    </TableCell>
                    <TableCell className="hidden text-muted-foreground sm:table-cell">
                      {r.arrival ? (ARRIVAL[r.arrival] ?? r.arrival) : "–"}
                    </TableCell>
                    <TableCell className="hidden sm:table-cell">
                      <TimeAgo at={r.receivedAtMs} />
                    </TableCell>
                    <TableCell>
                      <ProofCell receipt={r} />
                    </TableCell>
                    <TableCell>
                      {r.applied ? (
                        // The normal case is quiet; an exception stands out.
                        <span className="text-muted-foreground">Applied</span>
                      ) : (
                        <span className="text-foreground">Not applied</span>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )
        }
      </Loaded>
    </PageSection>
  );
}

export function BlocksPage() {
  return (
    <>
      <PageHeader
        eyebrow="Chain"
        title="Blocks"
        aside={
          <ExternalLink href={NOT_CHECKED_DOCS_URL} className="text-sm">
            Web node docs
          </ExternalLink>
        }
      />
      <BlockSearch />
      <BestChain />
      <Received />
    </>
  );
}
