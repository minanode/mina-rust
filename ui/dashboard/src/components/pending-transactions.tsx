/** Pending transactions, as the transaction pool lists them. `status`
 * adds a column with the node's view of each one. */

import * as React from "react";

import {
  tableRegion,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@minanode/design-system/components/table";
import { Amount } from "@minanode/design-system/patterns/amount";
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
            <TableCell className="text-right">
              <Amount
                unit="MINA"
                value={t.amount !== null ? formatMina(t.amount) : null}
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
            {status ? <TableCell>{status(t)}</TableCell> : null}
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
