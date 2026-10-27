import { ArrowLeft } from "lucide-react";

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
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { Signal } from "@minanode/design-system/patterns/signal";
import { Amount } from "@minanode/design-system/patterns/amount";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import {
  blockQuery,
  commandFees,
  decodeMemo,
  explorerBlockUrl,
  formatCount,
  formatHeight,
  formatMina,
  utcDateTime,
  type Block,
} from "@minanode/node-client";
import { useBlock, useNodeStatus } from "@minanode/node-client/react";
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
                        value={
                          c.kind === "Payment" ? formatMina(c.amount) : null
                        }
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
    </>
  );
}
