import * as React from "react";
import { Blocks, Search } from "lucide-react";

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
import { Amount } from "@minanode/design-system/patterns/amount";
import { Hash } from "@minanode/design-system/patterns/hash";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import { Signal, type Tone } from "@minanode/design-system/patterns/signal";
import {
  blockQuery,
  formatCount,
  formatHeight,
  formatMina,
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
import { Loaded, PolledAge } from "minanode-dashboard/components/states";

import { Link, navigate } from "../app/router";

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

export function ProofCell({ receipt }: { receipt: Receipt }) {
  const outcome = OUTCOME[receiptOutcome(receipt)];
  const time = proofTime(receipt.proof);
  return (
    <span className="flex items-center gap-2">
      <Signal tone={outcome.tone}>{outcome.label}</Signal>
      {time ? (
        <span className="font-mono text-xs text-muted-foreground tabular-nums">
          {time}
        </span>
      ) : null}
    </span>
  );
}

export const linkClass =
  "rounded-sm text-brand-text underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring";

/** The fees a header lists, commands' and SNARK work's, in MINA. */
function Fees({ h }: { h: BlockHeader }) {
  return (
    <Amount
      unit="MINA"
      value={h.transactionFees === null ? null : formatMina(h.transactionFees)}
    />
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
      className="flex max-w-xl flex-col gap-1.5"
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
      <label htmlFor={id} className="sr-only">
        Block height or state hash
      </label>
      <div className="flex gap-2">
        <Input
          id={id}
          value={value}
          placeholder="Height or state hash"
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
          Find
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

  return (
    <PageSection
      title="Best chain"
      description={
        chain.data?.k != null
          ? `The blocks this node holds, newest first: the root and up to ${formatCount(chain.data.k)} after it.`
          : "The blocks this node holds, newest first."
      }
      aside={<PolledAge polled={chain} staleAfterMs={POLL_MS * 4} />}
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
                    <TableHead className="text-right">zkApp</TableHead>
                    <TableHead className="text-right">SNARK work</TableHead>
                    <TableHead className="text-right">Fees</TableHead>
                    <TableHead>Proof check</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {c.headers.map(h => {
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
                            <Badge className="ml-2">best tip</Badge>
                          ) : null}
                          {h.height === c.rootHeight ? (
                            <Badge variant="outline" className="ml-2">
                              root
                            </Badge>
                          ) : null}
                        </TableCell>
                        <TableCell>
                          <TimeAgo at={h.timeMs} />
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums">
                          {formatCount(h.slotSinceGenesis)}
                        </TableCell>
                        <TableCell>
                          {h.producer ? (
                            <Hash value={h.producer} copy={false} />
                          ) : (
                            "–"
                          )}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums">
                          {formatCount(h.userCommands)}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums">
                          {formatCount(h.zkappCommands)}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums">
                          {formatCount(h.snarkJobs)}
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
              <div className="flex flex-wrap items-center gap-2">
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
                {c.rootHeight !== null &&
                c.headers[c.headers.length - 1].height > c.rootHeight ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    data-action="older"
                    onClick={() =>
                      setBelow(c.headers[c.headers.length - 1].height - 1)
                    }
                  >
                    Older
                  </Button>
                ) : null}
              </div>
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

  return (
    <PageSection
      title="Blocks received"
      description="Every block this node received, highest first, with the checks it ran on each in this tab."
      aside={<PolledAge polled={checks} staleAfterMs={POLL_MS * 4} />}
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
              className="[&_td]:align-middle [&_td]:whitespace-nowrap"
              containerProps={tableRegion("Blocks received")}
            >
              <TableHeader>
                <TableRow>
                  <TableHead>Height</TableHead>
                  <TableHead>State hash</TableHead>
                  <TableHead>Came by</TableHead>
                  <TableHead>Received</TableHead>
                  <TableHead>Proof</TableHead>
                  <TableHead>Chain</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {shownReceipts(v).map(r => (
                  <TableRow key={r.stateHash}>
                    <TableCell className="font-mono tabular-nums">
                      {formatHeight(r.height)}
                    </TableCell>
                    <TableCell>
                      <Hash value={r.stateHash} label="state hash" />
                    </TableCell>
                    <TableCell>
                      {r.arrival ? (ARRIVAL[r.arrival] ?? r.arrival) : "–"}
                    </TableCell>
                    <TableCell>
                      <TimeAgo at={r.receivedAtMs} />
                    </TableCell>
                    <TableCell>
                      <ProofCell receipt={r} />
                    </TableCell>
                    <TableCell>
                      {r.stateHash === tip ? (
                        <Badge>best tip</Badge>
                      ) : r.applied ? (
                        "Applied"
                      ) : (
                        <span className="text-muted-foreground">
                          Not applied
                        </span>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )
        }
      </Loaded>
      <p className="max-w-prose text-sm text-pretty text-muted-foreground">
        Blocks from the bootstrap show Not checked. The node fetched them by
        hash, back from a best tip whose proof it checked. Each block that
        arrives after that has its own proof checked.
      </p>
    </PageSection>
  );
}

export function BlocksPage() {
  return (
    <>
      <PageHeader
        eyebrow="Chain"
        title="Blocks"
        description="This node's best chain, and every block it received with the checks it ran in this tab."
      />
      <BlockSearch />
      <BestChain />
      <Received />
    </>
  );
}
