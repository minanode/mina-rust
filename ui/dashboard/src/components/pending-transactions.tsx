/** Pending transactions, as the transaction pool lists them. `status`
 * adds a column with the node's view of each one. */

import * as React from "react";
import { ArrowDown } from "lucide-react";

import {
  tableRegion,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@minanode/design-system/components/table";
import { Hash } from "@minanode/design-system/patterns/hash";
import { formatMina, type PendingTransaction } from "@minanode/node-client";

export function PendingTransactionsTable({
  list,
  status,
}: {
  list: PendingTransaction[];
  status?: (t: PendingTransaction) => React.ReactNode;
}) {
  return (
    // Nine columns: tighter inner cells, the card's inset kept at its edges.
    <Table
      className="[&_td]:px-3 [&_td]:align-middle [&_td]:whitespace-nowrap [&_td:first-child]:pl-4 [&_td:last-child]:pr-4 [&_th]:px-3 [&_th:first-child]:pl-4 [&_th:last-child]:pr-4"
      containerProps={tableRegion("Pending transactions")}
    >
      <TableHeader>
        <TableRow>
          <TableHead>Hash</TableHead>
          <TableHead>Kind</TableHead>
          <TableHead>From</TableHead>
          <TableHead>To</TableHead>
          <TableHead className="text-right">Amount (MINA)</TableHead>
          <TableHead className="text-right" aria-sort="descending">
            <span className="inline-flex items-center gap-1">
              Fee (MINA)
              <ArrowDown aria-hidden className="size-3" />
            </span>
          </TableHead>
          <TableHead className="text-right">Nonce</TableHead>
          <TableHead>Memo</TableHead>
          {status ? <TableHead>Status</TableHead> : null}
        </TableRow>
      </TableHeader>
      <TableBody>
        {list.map((t, i) => (
          <TableRow key={t.hash ?? i}>
            <TableCell>
              {t.hash ? <Hash value={t.hash} label="transaction hash" /> : "–"}
            </TableCell>
            <TableCell className="whitespace-nowrap">{t.kind}</TableCell>
            <TableCell>
              {t.sender ? <Hash value={t.sender} copy={false} /> : "–"}
            </TableCell>
            <TableCell>
              {t.receiver ? <Hash value={t.receiver} copy={false} /> : "–"}
            </TableCell>
            {/* The heads name the unit. */}
            <TableCell className="text-right font-mono tabular-nums">
              {t.amount !== null ? formatMina(t.amount) : "–"}
            </TableCell>
            <TableCell className="text-right font-mono tabular-nums">
              {t.fee !== null ? formatMina(t.fee) : "–"}
            </TableCell>
            <TableCell className="text-right font-mono tabular-nums">
              {t.nonce ?? "–"}
            </TableCell>
            <TableCell
              className="max-w-40 truncate text-muted-foreground"
              title={t.memo || undefined}
            >
              {t.memo || "–"}
            </TableCell>
            {status ? <TableCell>{status(t)}</TableCell> : null}
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
