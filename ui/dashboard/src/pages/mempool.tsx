import { Layers } from "lucide-react";

import { Card } from "@minanode/design-system/components/card";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { Stat } from "@minanode/design-system/patterns/stat";
import { formatCount, plural } from "@minanode/node-client";
import { useNodeStatus, useTransactionPool } from "@minanode/node-client/react";
import { PendingTransactionsTable } from "../components/pending-transactions";
import { Loaded, PolledAge } from "../components/states";

const POLL_MS = 5_000;

export function MempoolPage() {
  const { status } = useNodeStatus();
  const pool = useTransactionPool(POLL_MS);
  const txs = pool.data;
  const zkapps =
    txs?.filter(t => t.kind === "zkApp transaction").length ?? null;
  const snark = status.data?.snarkPool;

  return (
    <>
      <PageHeader
        eyebrow="Chain"
        title="Mempool"
        aside={<PolledAge polled={pool} staleAfterMs={POLL_MS * 4} />}
      />
      <Card className="overflow-hidden">
        {/* Two short cells side by side at every width, as Peers pairs
            its short ones. */}
        <dl className="grid grid-cols-2 divide-x">
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
            label="Pending"
            value={
              txs ? (
                <span
                  className={
                    txs.length === 0 ? "text-muted-foreground" : undefined
                  }
                >
                  {formatCount(txs.length)}
                </span>
              ) : (
                "–"
              )
            }
            detail={
              txs && zkapps !== null && txs.length > 0
                ? `${plural(txs.length - zkapps, "payment or delegation", "payments and delegations")}, ${plural(zkapps, "zkApp")}`
                : undefined
            }
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
            label="SNARK jobs"
            value={formatCount(snark?.jobs ?? null)}
            detail={
              snark?.snarks != null
                ? `${formatCount(snark.snarks)} with a proof`
                : "–"
            }
          />
        </dl>
      </Card>
      <PageSection title="Pending transactions">
        <Loaded polled={pool} what="the transaction pool" rows={6}>
          {list =>
            list.length === 0 ? (
              <EmptyState icon={<Layers />} title="The mempool is empty" />
            ) : (
              <PendingTransactionsTable list={list} />
            )
          }
        </Loaded>
      </PageSection>
    </>
  );
}
