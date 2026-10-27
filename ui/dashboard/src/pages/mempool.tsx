import { Layers } from "lucide-react";

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
import { Hash } from "@minanode/design-system/patterns/hash";
import { MetricCard } from "@minanode/design-system/patterns/metric-card";
import { Amount } from "@minanode/design-system/patterns/amount";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { formatCount, formatMina, plural } from "@minanode/node-client";
import { useNodeStatus, useTransactionPool } from "@minanode/node-client/react";
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
              <Table
                className="[&_td]:align-middle [&_td]:whitespace-nowrap"
                containerProps={tableRegion("Pending transactions")}
              >
                <TableHeader>
                  <TableRow>
                    <TableHead>Hash</TableHead>
                    <TableHead>Kind</TableHead>
                    <TableHead>From</TableHead>
                    <TableHead>To</TableHead>
                    <TableHead className="text-right">Amount</TableHead>
                    <TableHead className="text-right">Fee</TableHead>
                    <TableHead className="text-right">Nonce</TableHead>
                    <TableHead>Memo</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {list.map((t, i) => (
                    <TableRow key={t.hash ?? i}>
                      <TableCell>
                        {t.hash ? (
                          <Hash value={t.hash} label="transaction hash" />
                        ) : (
                          "–"
                        )}
                      </TableCell>
                      <TableCell className="whitespace-nowrap">
                        {t.kind}
                      </TableCell>
                      <TableCell>
                        {t.sender ? (
                          <Hash value={t.sender} copy={false} />
                        ) : (
                          "–"
                        )}
                      </TableCell>
                      <TableCell>
                        {t.receiver ? (
                          <Hash value={t.receiver} copy={false} />
                        ) : (
                          "–"
                        )}
                      </TableCell>
                      <TableCell className="text-right">
                        <Amount
                          unit="MINA"
                          value={
                            t.amount !== null ? formatMina(t.amount) : null
                          }
                        />
                      </TableCell>
                      <TableCell className="text-right">
                        <Amount
                          unit="MINA"
                          value={t.fee !== null ? formatMina(t.fee) : null}
                        />
                      </TableCell>
                      <TableCell className="text-right font-mono tabular-nums">
                        {t.nonce ?? "–"}
                      </TableCell>
                      <TableCell className="max-w-48 truncate text-muted-foreground">
                        {t.memo || "–"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )
          }
        </Loaded>
      </PageSection>
    </>
  );
}
