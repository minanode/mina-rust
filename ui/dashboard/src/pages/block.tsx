import { ArrowLeft } from "lucide-react";

import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { Amount } from "@minanode/design-system/patterns/amount";
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
import { Loaded } from "../components/states";

import { Link } from "../app/router";

const linkClass =
  "rounded-sm text-brand-text underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring";

export function BlockPage({ id }: { id: string }) {
  const query = blockQuery(id);
  const block = useBlock(query ? id : null);
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
              value: `${formatCount(b.slot)} (epoch ${formatCount(b.epoch)})`,
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
