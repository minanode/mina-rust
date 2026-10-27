import * as React from "react";
import { Network } from "lucide-react";

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
import { Signal } from "@minanode/design-system/patterns/signal";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { SegmentedControl } from "@minanode/design-system/patterns/segmented-control";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import {
  formatCount,
  formatHeight,
  peerCounts,
  peerHost,
  type Peer,
} from "@minanode/node-client";
import { usePeers } from "@minanode/node-client/react";
import { Loaded, PolledAge } from "../components/states";

const POLL_MS = 5_000;

function order(a: Peer, b: Peer) {
  if (a.connected !== b.connected) return a.connected ? -1 : 1;
  return (b.bestTipHeight ?? -1) - (a.bestTipHeight ?? -1);
}

export function PeersPage() {
  const peers = usePeers(POLL_MS);
  const [show, setShow] = React.useState<"connected" | "all">("connected");
  const counts = peers.data ? peerCounts(peers.data) : null;

  return (
    <>
      <PageHeader
        eyebrow="Network"
        title="Peers"
        description="The nodes this one knows, and the best tip each connected peer last sent."
        aside={<PolledAge polled={peers} staleAfterMs={POLL_MS * 4} />}
      />
      <div className="grid gap-4 sm:grid-cols-3">
        <MetricCard
          size="sm"
          value={formatCount(counts?.connected ?? null)}
          label="Connected"
          detail={`highest tip ${formatHeight(counts?.highestTip ?? null)}`}
        />
        <MetricCard
          size="sm"
          value={formatCount(counts?.connecting ?? null)}
          label="Connecting"
          detail="handshakes under way"
        />
        <MetricCard
          size="sm"
          value={formatCount(counts?.disconnected ?? null)}
          label="Known, not connected"
          detail="from discovery or past connections"
        />
      </div>
      <Loaded polled={peers} what="the peer list" rows={8}>
        {list => {
          const shown = (
            show === "connected" ? list.filter(p => p.connected) : list
          )
            .slice()
            .sort(order);
          return (
            <div className="flex flex-col gap-4">
              <SegmentedControl
                label="Which peers"
                options={[
                  {
                    value: "connected",
                    label: `Connected (${counts?.connected ?? 0})`,
                  },
                  { value: "all", label: `All (${list.length})` },
                ]}
                value={show}
                onValueChange={setShow}
              />
              {shown.length === 0 ? (
                <EmptyState
                  icon={<Network />}
                  title={
                    show === "connected"
                      ? "No peer is connected"
                      : "No peer known yet"
                  }
                  description={
                    <p>
                      The node keeps looking through its seeds and discovery.
                    </p>
                  }
                />
              ) : (
                <Table
                  className="[&_td]:align-middle [&_td]:whitespace-nowrap"
                  containerProps={tableRegion("Peers")}
                >
                  <TableHeader>
                    <TableRow>
                      <TableHead>Peer</TableHead>
                      <TableHead>Address</TableHead>
                      <TableHead>Transport</TableHead>
                      <TableHead>Direction</TableHead>
                      <TableHead className="text-right">Best tip</TableHead>
                      {show === "all" ? <TableHead>State</TableHead> : null}
                      <TableHead>
                        {show === "all" ? "Since" : "Connected"}
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {shown.map(p => (
                      <TableRow key={p.peerId}>
                        <TableCell>
                          <Hash value={p.peerId} label="peer ID" />
                        </TableCell>
                        <TableCell className="font-mono text-[0.8125rem] whitespace-nowrap">
                          {peerHost(p.address) ?? "–"}
                        </TableCell>
                        <TableCell>
                          {p.libp2p === null
                            ? "–"
                            : p.libp2p
                              ? "libp2p"
                              : "WebRTC"}
                        </TableCell>
                        <TableCell>
                          {p.incoming === null
                            ? "–"
                            : p.incoming
                              ? "Inbound"
                              : "Outbound"}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums">
                          {formatHeight(p.bestTipHeight)}
                        </TableCell>
                        {show === "all" ? (
                          <TableCell>
                            <Signal
                              tone={
                                p.connected
                                  ? "ok"
                                  : p.status === "Connecting"
                                    ? "info"
                                    : "neutral"
                              }
                            >
                              {p.status}
                            </Signal>
                          </TableCell>
                        ) : null}
                        <TableCell>
                          <TimeAgo at={p.sinceMs} />
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </div>
          );
        }}
      </Loaded>
    </>
  );
}
