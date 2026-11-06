import { Layers } from "lucide-react";

import { Card } from "@minanode/design-system/components/card";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { Signal } from "@minanode/design-system/patterns/signal";
import { Stat } from "@minanode/design-system/patterns/stat";
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
        eyebrow="Chain"
        title="Transaction pool"
        aside={<PolledAge polled={pool} staleAfterMs={POLL_MS * 4} />}
      />
      <Card className="gap-0 overflow-hidden py-0">
        <dl className="grid divide-y sm:grid-cols-3 sm:divide-x sm:divide-y-0">
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
              // At zero the count says it all, as Queued senders does.
              txs && txs.length === 0
                ? undefined
                : txs && zkapps !== null
                  ? `${plural(txs.length - zkapps, "payment or delegation", "payments and delegations")}, ${plural(zkapps, "zkApp")}`
                  : "–"
            }
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
            label="Queued senders"
            detail={
              txs && queuedSenders(txs) > 0 ? "taken in nonce order" : undefined
            }
            value={
              txs ? (
                <span
                  className={
                    queuedSenders(txs) === 0
                      ? "text-muted-foreground"
                      : undefined
                  }
                >
                  {formatCount(queuedSenders(txs))}
                </span>
              ) : (
                "–"
              )
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
                ? `${formatCount(snark.snarks)} with a proof on offer`
                : "–"
            }
          />
        </dl>
      </Card>
      <PageSection title="Pending transactions">
        <Loaded polled={pool} what="the transaction pool" rows={6}>
          {list => {
            if (list.length === 0) {
              return <EmptyState icon={<Layers />} title="The pool is empty" />;
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
                    <Signal tone="ok" variant="plain">
                      Ready
                    </Signal>
                  ) : (
                    <WaitsForNonce after={s.after} />
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

/** A pending transaction behind another nonce: two lines, as one doesn't
 * fit the column at desktop widths; the nonce whole and smaller, as it
 * qualifies the status. */
export function WaitsForNonce({ after }: { after: number }) {
  return (
    <Signal
      tone="neutral"
      variant="plain"
      className="items-start leading-snug [&>[data-slot=signal-dot]]:mt-[0.3rem]"
    >
      <span className="flex flex-col">
        <span>Waits for nonce</span>
        {/* As the Nonce column shows it. */}
        <span className="font-mono text-xs">{after}</span>
      </span>
    </Signal>
  );
}
