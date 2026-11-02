import { Layers } from "lucide-react";

import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { MetricCard } from "@minanode/design-system/patterns/metric-card";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { Signal } from "@minanode/design-system/patterns/signal";
import { formatCount, plural } from "@minanode/node-client";
import { useNodeStatus, useTransactionPool } from "@minanode/node-client/react";
import { PendingTransactionsTable } from "minanode-dashboard/components/pending-transactions";
import { Loaded, PolledAge } from "minanode-dashboard/components/states";

import { poolStatuses, queuedSenders } from "../lib/pool";

const POLL_MS = 5_000;

export function PoolPage() {
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
        title="Transaction pool"
        description="Transactions this node holds for the next blocks, as it heard them from its peers."
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
              : "Waiting for an answer"
          }
        />
        <MetricCard
          size="sm"
          value={txs ? formatCount(queuedSenders(txs)) : "–"}
          label="Senders with several pending"
          detail="Taken in nonce order"
        />
        <MetricCard
          size="sm"
          value={formatCount(snark?.jobs ?? null)}
          label="SNARK jobs"
          detail={
            snark?.snarks != null
              ? `${formatCount(snark.snarks)} with a proof on offer`
              : "Work the next blocks need proved"
          }
        />
      </div>
      <PageSection
        title="Pending transactions"
        description="Highest fee first."
      >
        <Loaded polled={pool} what="the transaction pool" rows={6}>
          {list => {
            if (list.length === 0) {
              return (
                <EmptyState
                  icon={<Layers />}
                  title="The pool is empty"
                  description={
                    <p>
                      Transactions show here when the node hears them, until a
                      block includes them.
                    </p>
                  }
                />
              );
            }
            const statuses = poolStatuses(list);
            const index = new Map(list.map((t, i) => [t, i]));
            return (
              <PendingTransactionsTable
                list={list}
                status={t => {
                  const s = statuses[index.get(t) ?? -1];
                  if (!s || s.kind === "unknown") return "–";
                  return s.kind === "next" ? (
                    <Signal tone="ok">Ready</Signal>
                  ) : (
                    <Signal tone="neutral">
                      Waits for nonce {formatCount(s.after)}
                    </Signal>
                  );
                }}
              />
            );
          }}
        </Loaded>
      </PageSection>
    </>
  );
}
