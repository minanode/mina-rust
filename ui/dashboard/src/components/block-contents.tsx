/** A block's transactions, zkApp transactions, SNARK work and fee
 * transfers, in tabs. */

import {
  tableRegion,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@minanode/design-system/components/table";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@minanode/design-system/components/tabs";
import { Amount } from "@minanode/design-system/patterns/amount";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { Hash } from "@minanode/design-system/patterns/hash";
import { Signal } from "@minanode/design-system/patterns/signal";
import { decodeMemo, formatMina, type Block } from "@minanode/node-client";

export function BlockContents({ block: b }: { block: Block }) {
  return (
    <Tabs defaultValue="commands">
      <TabsList aria-label="Block contents">
        <TabsTrigger value="commands">
          Transactions ({b.userCommands.length})
        </TabsTrigger>
        <TabsTrigger value="zkapps">
          zkApp transactions ({b.zkappCommands.length})
        </TabsTrigger>
        <TabsTrigger value="snarks">
          SNARK work ({b.snarkJobs.length})
        </TabsTrigger>
        <TabsTrigger value="fees">
          Fee transfers ({b.feeTransfers.length})
        </TabsTrigger>
      </TabsList>
      <TabsContent value="commands">
        {b.userCommands.length === 0 ? (
          <EmptyState title="No payments or delegations in this block" />
        ) : (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={tableRegion("Transactions")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Hash</TableHead>
                <TableHead>Kind</TableHead>
                <TableHead>From</TableHead>
                <TableHead>To</TableHead>
                <TableHead className="text-right">Amount</TableHead>
                <TableHead className="text-right">Fee</TableHead>
                <TableHead>Memo</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {b.userCommands.map((c, i) => (
                <TableRow key={c.hash ?? i}>
                  <TableCell>
                    {c.hash ? (
                      <Hash value={c.hash} label="transaction hash" />
                    ) : (
                      "–"
                    )}
                  </TableCell>
                  <TableCell>
                    {c.kind}
                    {c.failure ? (
                      <Signal tone="warn" className="ml-2" title={c.failure}>
                        failed
                      </Signal>
                    ) : null}
                  </TableCell>
                  <TableCell>
                    {c.from ? <Hash value={c.from} copy={false} /> : "–"}
                  </TableCell>
                  <TableCell>
                    {c.to ? <Hash value={c.to} copy={false} /> : "–"}
                  </TableCell>
                  <TableCell className="text-right">
                    <Amount
                      unit="MINA"
                      value={c.kind === "Payment" ? formatMina(c.amount) : null}
                    />
                  </TableCell>
                  <TableCell className="text-right">
                    <Amount unit="MINA" value={formatMina(c.fee)} />
                  </TableCell>
                  <TableCell className="max-w-48 truncate text-muted-foreground">
                    {decodeMemo(c.memo) || "–"}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </TabsContent>
      <TabsContent value="zkapps">
        {b.zkappCommands.length === 0 ? (
          <EmptyState title="No zkApp transactions in this block" />
        ) : (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={tableRegion("zkApp transactions")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Hash</TableHead>
                <TableHead>Fee payer</TableHead>
                <TableHead className="text-right">Account updates</TableHead>
                <TableHead className="text-right">Fee</TableHead>
                <TableHead>Result</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {b.zkappCommands.map((c, i) => (
                <TableRow key={c.hash ?? i}>
                  <TableCell>
                    {c.hash ? (
                      <Hash value={c.hash} label="command hash" />
                    ) : (
                      "–"
                    )}
                  </TableCell>
                  <TableCell>
                    {c.feePayer ? (
                      <Hash value={c.feePayer} copy={false} />
                    ) : (
                      "–"
                    )}
                  </TableCell>
                  <TableCell className="text-right font-mono tabular-nums">
                    {c.accountUpdates}
                  </TableCell>
                  <TableCell className="text-right">
                    <Amount unit="MINA" value={formatMina(c.fee)} />
                  </TableCell>
                  <TableCell>
                    <Signal tone={c.failed ? "warn" : "ok"}>
                      {c.failed ? "Failed" : "Applied"}
                    </Signal>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </TabsContent>
      <TabsContent value="snarks">
        {b.snarkJobs.length === 0 ? (
          <EmptyState title="This block bought no SNARK work" />
        ) : (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={tableRegion("SNARK work")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Prover</TableHead>
                <TableHead className="text-right">Fee</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {b.snarkJobs.map((j, i) => (
                <TableRow key={i}>
                  <TableCell>
                    {j.prover ? (
                      <Hash value={j.prover} label="prover key" />
                    ) : (
                      "–"
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <Amount unit="MINA" value={formatMina(j.fee)} />
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </TabsContent>
      <TabsContent value="fees">
        {b.feeTransfers.length === 0 ? (
          <EmptyState title="No fee transfers in this block" />
        ) : (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={tableRegion("Fee transfers")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Recipient</TableHead>
                <TableHead>Kind</TableHead>
                <TableHead className="text-right">Fee</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {b.feeTransfers.map((f, i) => (
                <TableRow key={i}>
                  <TableCell>
                    {f.recipient ? (
                      <Hash value={f.recipient} label="recipient key" />
                    ) : (
                      "–"
                    )}
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {f.type ?? "–"}
                  </TableCell>
                  <TableCell className="text-right">
                    <Amount unit="MINA" value={formatMina(f.fee)} />
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </TabsContent>
    </Tabs>
  );
}
