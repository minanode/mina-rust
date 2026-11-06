import { ArrowLeft } from "lucide-react";

import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { Amount, Measure } from "@minanode/design-system/patterns/amount";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import {
  blockQuery,
  commandFees,
  explorerBlockUrl,
  formatCount,
  formatHeight,
  formatMina,
  utcDateTime,
  type Block,
} from "@minanode/node-client";
import { useBlock, useNodeStatus } from "@minanode/node-client/react";
import { BlockContents } from "../components/block-contents";
import { TABLE_LINK } from "../components/cells";
import { Loaded } from "../components/states";

import { Link } from "../app/router";

export function BlockPage({ id }: { id: string }) {
  const query = blockQuery(id);
  const block = useBlock(query ? id : null);
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
      {query === null ? (
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
                title="No such block"
                description={<p>The node doesn't have it.</p>}
              />
            ) : (
              <BlockDetail block={b} />
            )
          }
        </Loaded>
      )}
    </>
  );
}

function BlockDetail({ block: b }: { block: Block }) {
  const { status } = useNodeStatus();
  const url = explorerBlockUrl(status.data?.network ?? null, b.height);
  const fees = commandFees(b);
  return (
    <>
      <div className="grid gap-6 lg:grid-cols-2">
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
                // A link to its page, cut and copied as the other hashes.
                <span className="inline-flex items-center gap-1">
                  <Link
                    to={`/blocks/${b.previousStateHash}`}
                    className={TABLE_LINK}
                  >
                    {`${b.previousStateHash.slice(0, 6)}…${b.previousStateHash.slice(-6)}`}
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
                  {formatCount(b.slot)}{" "}
                  <span className="font-sans text-muted-foreground">
                    epoch {formatCount(b.epoch)}
                  </span>
                </span>
              ),
            },
            {
              label: "Slot since genesis",
              value: formatCount(b.slotSinceGenesis),
            },
          ]}
        />
        <KeyValueList
          title="Rewards and ledgers"
          rows={[
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
              label: "Coinbase receiver",
              value: b.coinbaseReceiver ? (
                <Hash
                  value={b.coinbaseReceiver}
                  label="coinbase receiver"
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
                  value={fees === null ? null : formatMina(fees)}
                />
              ),
            },
            {
              label: "Snarked ledger",
              value: b.snarkedLedgerHash ? (
                <Hash
                  value={b.snarkedLedgerHash}
                  label="snarked ledger hash"
                  className="sm:justify-end"
                />
              ) : (
                "–"
              ),
            },
            {
              label: "Staged ledger",
              value: b.stagedLedgerHash ? (
                <Hash
                  value={b.stagedLedgerHash}
                  label="staged ledger hash"
                  className="sm:justify-end"
                />
              ) : (
                "–"
              ),
            },
          ]}
        />
      </div>

      <BlockContents block={b} />
    </>
  );
}
