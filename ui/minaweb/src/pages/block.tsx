import { ArrowLeft } from "lucide-react";

import { Amount, Measure } from "@minanode/design-system/patterns/amount";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { StatRow } from "@minanode/design-system/patterns/stat-row";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import { Signal } from "@minanode/design-system/patterns/signal";
import {
  blockQuery,
  explorerBlockUrl,
  formatCount,
  formatHeight,
  formatMina,
  receiptOutcome,
  shortHash,
  type BlockDetail,
  type Check,
  type Receipt,
  utcDateTime,
} from "@minanode/node-client";
import {
  useChainBlock,
  useNodeStatus,
  useVerificationOf,
} from "@minanode/node-client/react";
import { BlockContents } from "minanode-dashboard/components/block-contents";
import { Loaded } from "minanode-dashboard/components/states";

import { Link } from "../app/router";
import { ARRIVAL, linkClass, NOT_CHECKED_DOCS_URL } from "./blocks";

const CHECK: Record<Check["status"], string> = {
  passed: "Passed",
  failed: "Failed",
  pending: "Checking",
  skipped: "Skipped",
  notRun: "Not run",
};

function checkValue(check: Check) {
  const tone =
    check.status === "passed"
      ? "ok"
      : check.status === "failed"
        ? "error"
        : check.status === "pending"
          ? "info"
          : "neutral";
  // One run, flush right like every other value: the state, then its time.
  return (
    <span className="inline-flex items-center justify-end gap-2">
      <Signal tone={tone} variant="plain">
        {CHECK[check.status]}
      </Signal>
      {check.status === "passed" && check.elapsedMs !== null ? (
        <Measure
          className="text-muted-foreground"
          value={
            check.elapsedMs < 1 ? "<1 ms" : `${Math.round(check.elapsedMs)} ms`
          }
        />
      ) : null}
    </span>
  );
}

/** The node's own checks of the block, from its verification receipt. */
function NodeCheck({ hash }: { hash: string }) {
  const checks = useVerificationOf([hash], 10_000);
  const receipt: Receipt | null =
    checks.data?.receipts.find(r => r.stateHash === hash) ?? null;
  return (
    <Loaded polled={checks} what="this node's checks" rows={4}>
      {() =>
        receipt === null ? (
          <KeyValueList
            className="h-full"
            title="This node's check"
            footer="The node keeps a record of the blocks it received lately. It has none for this one: it came in a bootstrap before the records start, or the record has aged out."
            rows={[{ label: "Proof", value: "No record" }]}
          />
        ) : (
          <KeyValueList
            className="h-full"
            title="This node's check"
            data-outcome={receiptOutcome(receipt)}
            footer={
              receipt.proof.status === "notRun" ? (
                <ExternalLink href={NOT_CHECKED_DOCS_URL}>
                  Web node docs
                </ExternalLink>
              ) : undefined
            }
            rows={[
              { label: "Block proof", value: checkValue(receipt.proof) },
              { label: "Delta chain", value: checkValue(receipt.deltaChain) },
              { label: "Body hash", value: checkValue(receipt.body) },
              {
                label: "Came by",
                value: receipt.arrival
                  ? (ARRIVAL[receipt.arrival] ?? receipt.arrival)
                  : "–",
                prose: true,
              },
              {
                label: "Received",
                value: <TimeAgo at={receipt.receivedAtMs} />,
              },
              {
                label: "Applied",
                value: receipt.applied ? "Yes" : "No",
                prose: true,
              },
            ]}
          />
        )
      }
    </Loaded>
  );
}

/** A large figure's unit: smaller and lighter, as the overview's. */
const FIGURE_UNIT = "font-sans text-sm font-normal";

/** A zero amount, muted so the others stand out. */
function zero(nanomina: bigint | null): string | undefined {
  return nanomina === 0n ? "text-muted-foreground" : undefined;
}

function Detail({ block: b }: { block: BlockDetail }) {
  const { status } = useNodeStatus();
  const url = explorerBlockUrl(status.data?.network ?? null, b.height);
  const h = b.header;
  return (
    <>
      <div
        className="grid gap-6 lg:grid-cols-2"
        data-block-height={b.height ?? ""}
        data-block-hash={b.stateHash}
      >
        <KeyValueList
          className="h-full"
          title="Block"
          aside={
            url ? (
              <ExternalLink href={url} className="font-sans text-sm">
                Explorer
              </ExternalLink>
            ) : null
          }
          rows={[
            {
              label: "State hash",
              value: (
                <Hash
                  value={b.stateHash}
                  label="state hash"
                  className="sm:justify-end"
                />
              ),
            },
            {
              label: "Previous",
              value: b.previousStateHash ? (
                // A link to its page, and copied as the other hashes are.
                <span className="inline-flex items-center gap-1 sm:justify-end">
                  <Link
                    to={`/blocks/${b.previousStateHash}`}
                    className={`font-mono ${linkClass}`}
                  >
                    {shortHash(b.previousStateHash)}
                  </Link>
                  <CopyButton
                    text={b.previousStateHash}
                    label="previous state hash"
                    className="-me-1"
                  />
                </span>
              ) : (
                "–"
              ),
            },
            { label: "Height", value: formatHeight(b.height), tone: "brand" },
            {
              label: "Time",
              value: (
                <span>
                  <Measure value={utcDateTime(b.timeMs)} />{" "}
                  <TimeAgo at={b.timeMs} className="text-muted-foreground" />
                </span>
              ),
            },
            {
              label: "Slot",
              value: (
                <span>
                  {formatCount(b.slotSinceGenesis)}{" "}
                  <span className="font-sans text-muted-foreground">
                    epoch {formatCount(b.epoch)}
                  </span>
                </span>
              ),
            },
            {
              label: "Producer",
              value: b.creator ? (
                <Hash
                  value={b.creator}
                  label="producer key"
                  className="sm:justify-end"
                />
              ) : (
                "–"
              ),
            },
          ]}
        />
        <NodeCheck hash={b.stateHash} />
      </div>
      {/* What the block paid, in one strip like the overview's, so the two
          lists above are of a length. */}
      <StatRow
        data-block-economics
        label="What the block paid"
        items={[
          {
            label: "Coinbase",
            value: (
              <Amount
                unit="MINA"
                unitClassName={FIGURE_UNIT}
                value={formatMina(b.coinbase)}
              />
            ),
          },
          {
            label: "Transaction fees",
            value: (
              <Amount
                unit="MINA"
                unitClassName={FIGURE_UNIT}
                className={zero(h.transactionFees)}
                value={
                  h.transactionFees === null
                    ? null
                    : formatMina(h.transactionFees)
                }
              />
            ),
          },
          {
            label: "SNARK fees",
            value: (
              <Amount
                unit="MINA"
                unitClassName={FIGURE_UNIT}
                className={zero(h.snarkFees)}
                value={h.snarkFees === null ? null : formatMina(h.snarkFees)}
              />
            ),
          },
          {
            label: "Ledger proof",
            value: h.proofEmitted ? (
              <span className="font-sans">Emitted</span>
            ) : (
              <span className="font-sans text-muted-foreground">None</span>
            ),
          },
        ]}
      />
      <BlockContents block={b} />
    </>
  );
}

export function BlockPage({ id }: { id: string }) {
  const valid = blockQuery(id) !== null;
  const block = useChainBlock(valid ? id.trim() : null);
  const title = /^\d+$/.test(id) ? `Block #${id}` : "Block";

  return (
    <>
      <PageHeader
        eyebrow={
          // The way back sits where the other pages' eyebrow does, so the
          // title lines up with theirs.
          <Link
            to="/blocks"
            className="inline-flex items-center gap-1 rounded-sm leading-none underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring"
          >
            <ArrowLeft aria-hidden className="size-3" />
            Blocks
          </Link>
        }
        title={
          block.data?.height
            ? `Block ${formatHeight(block.data.height)}`
            : title
        }
      />
      {!valid ? (
        <EmptyState
          title="That isn't a block height or a state hash"
          description={
            <p>
              A height is a number; a state hash starts with 3N and has 52
              characters.
            </p>
          }
        />
      ) : (
        <Loaded polled={block} what="this block" rows={6}>
          {b =>
            b === null ? (
              <EmptyState
                title="Not in this node's frontier"
                description={
                  <p>
                    The node holds the root and the blocks after it on its best
                    chain. This block is older, on another fork, or not here
                    yet.
                  </p>
                }
              />
            ) : (
              <Detail block={b} />
            )
          }
        </Loaded>
      )}
    </>
  );
}
