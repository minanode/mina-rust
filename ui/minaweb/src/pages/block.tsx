import { ArrowLeft } from "lucide-react";

import { Amount } from "@minanode/design-system/patterns/amount";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import { Signal } from "@minanode/design-system/patterns/signal";
import {
  blockQuery,
  explorerBlockUrl,
  formatCount,
  formatHeight,
  formatMina,
  receiptOutcome,
  utcDateTime,
  type BlockDetail,
  type Check,
  type Receipt,
} from "@minanode/node-client";
import {
  useChainBlock,
  useNodeStatus,
  useVerificationOf,
} from "@minanode/node-client/react";
import { BlockContents } from "minanode-dashboard/components/block-contents";
import { Loaded } from "minanode-dashboard/components/states";

import { Link } from "../app/router";
import { ARRIVAL, linkClass, NOT_CHECKED_DOCS_URL, OUTCOME } from "./blocks";

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
  return (
    <span className="inline-flex items-center gap-2">
      <Signal tone={tone}>{CHECK[check.status]}</Signal>
      {check.status === "passed" && check.elapsedMs !== null ? (
        <span className="font-mono text-xs text-muted-foreground tabular-nums">
          {Math.round(check.elapsedMs)} ms
        </span>
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
            title="This node's check"
            footer="The node keeps a record of the blocks it received lately. It has none for this one: it came in a bootstrap before the records start, or the record has aged out."
            rows={[{ label: "Proof", value: "No record" }]}
          />
        ) : (
          <KeyValueList
            title="This node's check"
            aside={
              <Signal tone={OUTCOME[receiptOutcome(receipt)].tone}>
                {OUTCOME[receiptOutcome(receipt)].label}
              </Signal>
            }
            footer={
              receipt.proof.status === "notRun" ? (
                <ExternalLink href={NOT_CHECKED_DOCS_URL}>
                  Web node docs
                </ExternalLink>
              ) : (
                "Checked in this tab, by the node's own verifier."
              )
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
              },
              {
                label: "Received",
                value: <TimeAgo at={receipt.receivedAtMs} />,
              },
              {
                label: "Applied",
                value: receipt.applied ? "Yes" : "No",
              },
            ]}
          />
        )
      }
    </Loaded>
  );
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
                <Link
                  to={`/blocks/${b.previousStateHash}`}
                  className={`font-mono ${linkClass}`}
                >
                  {`${b.previousStateHash.slice(0, 8)}…${b.previousStateHash.slice(-6)}`}
                </Link>
              ) : (
                "–"
              ),
            },
            { label: "Height", value: formatHeight(b.height), tone: "brand" },
            {
              label: "Time",
              value: (
                <span>
                  {utcDateTime(b.timeMs)} (<TimeAgo at={b.timeMs} />)
                </span>
              ),
            },
            {
              label: "Slot",
              value: `${formatCount(b.slotSinceGenesis)} (epoch ${formatCount(b.epoch)})`,
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
            {
              label: "Coinbase",
              value: <Amount unit="MINA" value={formatMina(b.coinbase)} />,
            },
            {
              label: "Transaction fees",
              value: (
                <Amount
                  unit="MINA"
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
                  value={h.snarkFees === null ? null : formatMina(h.snarkFees)}
                />
              ),
            },
            {
              label: "Ledger proof",
              value: h.proofEmitted ? "Emitted" : "None",
            },
          ]}
        />
        <NodeCheck hash={b.stateHash} />
      </div>
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
      <div>
        <Link
          to="/blocks"
          className={`inline-flex items-center gap-1.5 text-sm ${linkClass}`}
        >
          <ArrowLeft aria-hidden className="size-4" />
          Blocks
        </Link>
      </div>
      <PageHeader
        eyebrow="Chain"
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
