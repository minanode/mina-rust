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
import { Hash } from "@minanode/design-system/patterns/hash";
import { Signal } from "@minanode/design-system/patterns/signal";
import { decodeMemo, formatMina, type Block } from "@minanode/node-client";

import { CountOf, PhoneList, type PhoneRow } from "./phone-list";

/** A tab with nothing in it: one quiet line where the table would be. */
function Nothing({ children }: { children: React.ReactNode }) {
  return <p className="px-4 py-5 text-sm text-muted-foreground">{children}</p>;
}

/** A tab's table, inside the panel: no box of its own. */
function inPanel(label: string) {
  return { ...tableRegion(label), className: "rounded-none border-0" };
}

/** The node's fee transfer kinds, in words. */
const FEE_TRANSFER_TYPE: Record<string, string> = {
  Fee_transfer: "Fee transfer",
  Fee_transfer_via_coinbase: "Via coinbase",
};

/** The amounts' heads name the unit (MINA). */
export function BlockContents({ block: b }: { block: Block }) {
  return (
    <>
      <PhoneContents block={b} />
      <TabbedContents block={b} />
    </>
  );
}

/** Small mono key in a phone row's meta line. */
function Key({ value }: { value: string | null }) {
  return value ? (
    <Hash value={value} copy={false} className="text-xs text-foreground" />
  ) : (
    <>–</>
  );
}

/** A fee in MINA beside its unit, as phone rows set it. */
function Fee({ nanomina }: { nanomina: Parameters<typeof formatMina>[0] }) {
  return (
    <Amount
      unit="MINA"
      value={formatMina(nanomina) ?? "–"}
      className="text-foreground"
    />
  );
}

/** Below 640 px: the four parts stacked, each with its count, a row an
 * item, so no column or tab is cut at the panel's edge. */
function PhoneContents({ block: b }: { block: Block }) {
  const part = (
    title: string,
    count: number,
    empty: string,
    rows: PhoneRow[],
  ) => (
    <section className="border-t first:border-t-0">
      <h3 className="px-4 pt-3 pb-1 text-sm font-medium text-foreground">
        {title} <span className="text-muted-foreground">({count})</span>
      </h3>
      {rows.length === 0 ? (
        <p className="px-4 pb-3 text-sm text-muted-foreground">{empty}</p>
      ) : (
        <PhoneList
          label={title}
          rows={rows}
          className="rounded-none border-0 bg-transparent"
        />
      )}
    </section>
  );
  return (
    <div className="flex flex-col rounded-xl border bg-card sm:hidden">
      {part(
        "Transactions",
        b.userCommands.length,
        "No payments or delegations in this block",
        b.userCommands.map((c, i) => ({
          key: c.hash ?? i,
          lead: (
            <span className="inline-flex items-center gap-2">
              {c.hash ? <Hash value={c.hash} label="transaction hash" /> : "–"}
              {c.failure ? (
                <Signal variant="plain" tone="warn" title={c.failure}>
                  failed
                </Signal>
              ) : null}
            </span>
          ),
          end:
            c.kind === "Payment" ? (
              <Amount unit="MINA" value={formatMina(c.amount) ?? "–"} />
            ) : (
              <span className="text-muted-foreground">{c.kind}</span>
            ),
          meta: [
            <>
              from <Key value={c.from} />
            </>,
            <>
              to <Key value={c.to} />
            </>,
          ],
          more: [
            <>
              fee <Fee nanomina={c.fee} />
            </>,
            decodeMemo(c.memo) || null,
          ],
        })),
      )}
      {part(
        "zkApp transactions",
        b.zkappCommands.length,
        "No zkApp transactions in this block",
        b.zkappCommands.map((c, i) => ({
          key: c.hash ?? i,
          lead: c.hash ? <Hash value={c.hash} label="command hash" /> : "–",
          end: (
            <Signal variant="plain" tone={c.failed ? "warn" : "ok"}>
              {c.failed ? "Failed" : "Applied"}
            </Signal>
          ),
          meta: [
            <>
              fee payer <Key value={c.feePayer} />
            </>,
            <CountOf key="u" n={c.accountUpdates} one="account update" />,
            <>
              fee <Fee nanomina={c.fee} />
            </>,
          ],
        })),
      )}
      {part(
        "SNARK work",
        b.snarkJobs.length,
        "No SNARK work in this block",
        b.snarkJobs.map((j, i) => ({
          key: i,
          lead: j.prover ? <Hash value={j.prover} label="prover key" /> : "–",
          end: <Amount unit="MINA" value={formatMina(j.fee) ?? "–"} />,
          meta: [],
        })),
      )}
      {part(
        "Fee transfers",
        b.feeTransfers.length,
        "No fee transfers in this block",
        b.feeTransfers.map((f, i) => ({
          key: i,
          lead: f.recipient ? (
            <Hash value={f.recipient} label="recipient key" />
          ) : (
            "–"
          ),
          end: <Amount unit="MINA" value={formatMina(f.fee) ?? "–"} />,
          meta: [f.type ? (FEE_TRANSFER_TYPE[f.type] ?? f.type) : null],
        })),
      )}
    </div>
  );
}

/** From 640 px: the four parts as tabs over their tables. */
function TabbedContents({ block: b }: { block: Block }) {
  return (
    // One panel: the tabs on top, the table beneath them.
    <Tabs
      defaultValue="commands"
      className="hidden gap-0 overflow-hidden rounded-xl border bg-card sm:flex"
    >
      {/* The tabs' own padding puts their text on the table's 16 px edge;
          the focus ring sits inside each tab, clear of the panel's edge. */}
      <TabsList aria-label="Block contents" className="px-2 pt-1">
        <TabsTrigger value="commands">
          Transactions{" "}
          <span className="text-muted-foreground">
            ({b.userCommands.length})
          </span>
        </TabsTrigger>
        <TabsTrigger value="zkapps">
          zkApp transactions{" "}
          <span className="text-muted-foreground">
            ({b.zkappCommands.length})
          </span>
        </TabsTrigger>
        <TabsTrigger value="snarks">
          SNARK work{" "}
          <span className="text-muted-foreground">({b.snarkJobs.length})</span>
        </TabsTrigger>
        <TabsTrigger value="fees">
          Fee transfers{" "}
          <span className="text-muted-foreground">
            ({b.feeTransfers.length})
          </span>
        </TabsTrigger>
      </TabsList>
      <TabsContent value="commands">
        {b.userCommands.length === 0 ? (
          <Nothing>No payments or delegations in this block</Nothing>
        ) : (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={inPanel("Transactions")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Hash</TableHead>
                <TableHead>Kind</TableHead>
                <TableHead>From</TableHead>
                <TableHead>To</TableHead>
                <TableHead className="text-right">Amount (MINA)</TableHead>
                <TableHead className="text-right">Fee (MINA)</TableHead>
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
                    {c.kind === "Payment" ? (
                      <span className="font-mono tabular-nums">
                        {formatMina(c.amount)}
                      </span>
                    ) : (
                      <span className="text-muted-foreground">–</span>
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <span className="font-mono tabular-nums">
                      {formatMina(c.fee)}
                    </span>
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
          <Nothing>No zkApp transactions in this block</Nothing>
        ) : (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={inPanel("zkApp transactions")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Hash</TableHead>
                <TableHead>Fee payer</TableHead>
                <TableHead className="text-right">Account updates</TableHead>
                <TableHead className="text-right">Fee (MINA)</TableHead>
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
                    <span className="font-mono tabular-nums">
                      {formatMina(c.fee)}
                    </span>
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
          <Nothing>No SNARK work in this block</Nothing>
        ) : (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={inPanel("SNARK work")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Prover</TableHead>
                <TableHead className="text-right">Fee (MINA)</TableHead>
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
                    <span className="font-mono tabular-nums">
                      {formatMina(j.fee)}
                    </span>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </TabsContent>
      <TabsContent value="fees">
        {b.feeTransfers.length === 0 ? (
          <Nothing>No fee transfers in this block</Nothing>
        ) : (
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={inPanel("Fee transfers")}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Recipient</TableHead>
                <TableHead>Kind</TableHead>
                <TableHead className="text-right">Fee (MINA)</TableHead>
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
                  <TableCell>
                    {f.type ? (FEE_TRANSFER_TYPE[f.type] ?? f.type) : "–"}
                  </TableCell>
                  <TableCell className="text-right">
                    <span className="font-mono tabular-nums">
                      {formatMina(f.fee)}
                    </span>
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
