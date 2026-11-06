import * as React from "react";
import { Network } from "lucide-react";

import { Card } from "@minanode/design-system/components/card";
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
import { Signal } from "@minanode/design-system/patterns/signal";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { SegmentedControl } from "@minanode/design-system/patterns/segmented-control";
import { Stat } from "@minanode/design-system/patterns/stat";
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

/** A count, muted at zero, as zeros are in the tables. */
function Muted0({ n }: { n: number | null }) {
  return (
    <span className={n === 0 ? "text-muted-foreground" : undefined}>
      {formatCount(n)}
    </span>
  );
}

/** A peer's state as a dot and a word. */
function PeerState({ peer: p }: { peer: Peer }) {
  return (
    <Signal
      variant="plain"
      tone={p.connected ? "ok" : p.status === "Connecting" ? "info" : "neutral"}
    >
      {p.status}
    </Signal>
  );
}

/** The peers on a phone: a row each, the id and best tip, then where and
 * since when. */
function PeerList({ peers, all }: { peers: Peer[]; all: boolean }) {
  return (
    <ul
      aria-label="Peers"
      className="flex flex-col divide-y rounded-xl border bg-card sm:hidden"
    >
      {peers.map(p => (
        <li key={p.peerId} className="flex flex-col gap-1 px-4 py-3">
          <div className="flex items-center justify-between gap-3 text-sm">
            <Hash value={p.peerId} copy={false} />
            <span className="font-mono text-[0.8125rem] tabular-nums">
              {formatHeight(p.bestTipHeight)}
            </span>
          </div>
          <p className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
            <span className="min-w-0 truncate font-mono">
              {peerHost(p.address) ?? "–"}
            </span>
            <span className="flex shrink-0 items-center gap-2">
              {all ? <PeerState peer={p} /> : null}
              <TimeAgo at={p.sinceMs} />
            </span>
          </p>
        </li>
      ))}
    </ul>
  );
}

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
        eyebrow="Node"
        title="Peers"
        aside={<PolledAge polled={peers} staleAfterMs={POLL_MS * 4} />}
      />
      <Card className="overflow-hidden">
        {/* On a phone, Connected across the top and the other two beside
            each other below it. */}
        <dl className="grid grid-cols-2 sm:grid-cols-3">
          <Stat
            variant="labelled"
            inList
            className="col-span-2 border-b px-4 py-3 sm:col-span-1 sm:border-r sm:border-b-0 sm:py-5"
            label="Connected"
            value={formatCount(counts?.connected ?? null)}
            detail={
              counts?.highestTip != null
                ? `highest tip ${formatHeight(counts.highestTip)}`
                : "–"
            }
          />
          <Stat
            variant="labelled"
            inList
            className="border-r px-4 py-3 sm:py-5"
            label="Connecting"
            value={<Muted0 n={counts?.connecting ?? null} />}
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-3 sm:py-5"
            label="Not connected"
            value={<Muted0 n={counts?.disconnected ?? null} />}
          />
        </dl>
      </Card>
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
                <>
                  <PeerList peers={shown} all={show === "all"} />
                  <Table
                    className="[&_td]:align-middle [&_td]:whitespace-nowrap"
                    containerProps={{
                      ...tableRegion("Peers"),
                      className: "hidden sm:block",
                    }}
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
                              <PeerState peer={p} />
                            </TableCell>
                          ) : null}
                          <TableCell>
                            <TimeAgo at={p.sinceMs} />
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </>
              )}
            </div>
          );
        }}
      </Loaded>
    </>
  );
}
