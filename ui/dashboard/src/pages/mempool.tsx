import { Layers } from "lucide-react";

import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { MetricCard } from "@minanode/design-system/patterns/metric-card";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
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
        eyebrow="Pools"
        title="Mempool"
        description="Transactions this node holds for the next blocks, and the SNARK work on offer."
        aside={<PolledAge polled={pool} staleAfterMs={POLL_MS * 4} />}
      />
      <div className="grid gap-4 sm:grid-cols-3">
        <MetricCard
          size="sm"
          value={txs ? formatCount(txs.length) : "–"}
          label="Pending transactions"
          detail={
            txs && zkapps !== null
              ? `${plural(txs.length - zkapps, "payment or delegation", "payments and delegations")}, ${plural(zkapps, "zkApp transaction")}`
              : "Waiting for a block"
          }
        />
        <MetricCard
          size="sm"
          value={formatCount(snark?.jobs ?? null)}
          label="SNARK jobs"
          detail="Work the next blocks need proved"
        />
        <MetricCard
          size="sm"
          value={formatCount(snark?.snarks ?? null)}
          label="Jobs with a proof"
          detail={
            snark?.jobs !== null && snark?.jobs !== undefined
              ? `of ${formatCount(snark.jobs)}, offered by provers for a fee`
              : "Offered by provers, for a fee"
          }
        />
      </div>
      <PageSection
        title="Pending transactions"
        description="Highest fee first, as a block producer takes them."
      >
        <Loaded polled={pool} what="the transaction pool" rows={6}>
          {list =>
            list.length === 0 ? (
              <EmptyState
                icon={<Layers />}
                title="The mempool is empty"
                description={
                  <p>
                    Transactions appear here when the node receives them, until
                    a block includes them.
                  </p>
                }
              />
            ) : (
              <PendingTransactionsTable list={list} />
            )
          }
        </Loaded>
      </PageSection>
    </>
  );
}
