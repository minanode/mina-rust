import * as React from "react";

import { Measure } from "@minanode/design-system/patterns/amount";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import { Signal } from "@minanode/design-system/patterns/signal";
import { formatHeight, type Peer } from "@minanode/node-client";
import { usePeers, usePoll } from "@minanode/node-client/react";
import { PolledAge } from "minanode-dashboard/components/states";

import { useBoot } from "../app/boot-context";
import { formatBytes } from "../boot/failures";
import { CONFIG } from "../config";
import {
  noteLastSeen,
  otherPeers,
  parseRouteStats,
  relayRows,
  type RelayRow,
  type RelayState,
  type RouteStats,
} from "../lib/peers";
import { SharingCard } from "./sharing-card";

const POLL_MS = 5_000;

/** The minanode.org page that explains the connections in full. */
export const CONNECTIONS_DOCS_URL =
  "https://minanode.org/docs/web-node-connections/";

/** When each relay was last seen connected, for this tab's life. */
const lastSeen = new Map<string, number>();

const STATE: Record<
  RelayState,
  { label: string; tone: "ok" | "info" | "warn" }
> = {
  connected: { label: "Connected", tone: "ok" },
  connecting: { label: "Connecting", tone: "info" },
  retrying: { label: "Not connected, retrying", tone: "warn" },
};

function route(stats: RouteStats | null): string {
  if (stats?.route === "direct") return "Direct";
  if (stats?.route === "turn") return "Through TURN";
  return "–";
}

function trafficRows(stats: RouteStats | null) {
  return [
    { label: "Route", value: route(stats), prose: true },
    {
      label: "Round trip",
      value:
        stats?.rttMs != null ? (
          <Measure value={`${Math.round(stats.rttMs)} ms`} />
        ) : (
          "–"
        ),
    },
    {
      label: "Data in, out",
      value: stats ? (
        <>
          <Measure value={formatBytes(stats.bytesReceived)} />
          <span className="mr-1.5 font-sans text-muted-foreground">,</span>
          <Measure value={formatBytes(stats.bytesSent)} />
        </>
      ) : (
        "–"
      ),
    },
  ];
}

function RelayCard({ row }: { row: RelayRow }) {
  const { label, tone } = STATE[row.state];
  const connected = row.state === "connected";
  return (
    <KeyValueList
      data-relay={row.name}
      data-relay-state={row.state}
      data-route={row.stats?.route ?? ""}
      title={row.name}
      aside={<Signal tone={tone}>{label}</Signal>}
      rows={
        connected
          ? [
              ...trafficRows(row.stats),
              {
                label: "Best tip",
                value: formatHeight(row.peer?.bestTipHeight ?? null),
              },
              {
                label: "Since",
                value: row.sinceMs ? <TimeAgo at={row.sinceMs} /> : "–",
              },
            ]
          : [
              {
                label: "Last connected",
                value: row.sinceMs ? <TimeAgo at={row.sinceMs} /> : "–",
              },
            ]
      }
    />
  );
}

function OtherPeerCard({
  peer,
  stats,
}: {
  peer: Peer;
  stats: RouteStats | null;
}) {
  return (
    <KeyValueList
      data-other-peer=""
      title={<Hash value={peer.peerId} label="peer ID" />}
      rows={[
        ...trafficRows(stats),
        { label: "Best tip", value: formatHeight(peer.bestTipHeight) },
        {
          label: "Since",
          value: peer.sinceMs ? <TimeAgo at={peer.sinceMs} /> : "–",
        },
      ]}
    />
  );
}

export function PeersPage() {
  const boot = useBoot();
  const peers = usePeers(POLL_MS);
  const glue = boot.glue;
  const stats = usePoll(
    glue?.webrtc_peer_stats ? "webrtc-stats" : null,
    async () => parseRouteStats(await glue?.webrtc_peer_stats?.()),
    POLL_MS,
  );
  const relays = CONFIG.relays;
  const list = peers.data;
  React.useEffect(() => {
    if (list) noteLastSeen(lastSeen, relays, list, Date.now());
  }, [list, relays]);
  const routes = stats.data ?? new Map<string, RouteStats>();
  const rows = relayRows(relays, list ?? [], routes, lastSeen);
  const others = list ? otherPeers(relays, list) : [];

  return (
    <>
      <PageHeader
        eyebrow="Node"
        title="Peers"
        aside={
          <>
            <PolledAge polled={peers} staleAfterMs={POLL_MS * 4} />
            {/* The one link to the explanation. */}
            <ExternalLink href={CONNECTIONS_DOCS_URL} className="text-sm">
              Web node docs
            </ExternalLink>
          </>
        }
      />
      {relays.length ? (
        <div className="grid items-start gap-6 md:grid-cols-2">
          {rows.map(row => (
            <RelayCard key={row.name} row={row} />
          ))}
        </div>
      ) : null}
      <SharingCard
        on={boot.sharing}
        browsers={others.length}
        role={boot.role}
      />
      {others.length ? (
        <PageSection title={relays.length ? "Other browsers" : "Peers"}>
          <div className="grid items-start gap-6 md:grid-cols-2">
            {others.map(peer => (
              <OtherPeerCard
                key={peer.peerId}
                peer={peer}
                stats={routes.get(peer.peerId) ?? null}
              />
            ))}
          </div>
        </PageSection>
      ) : null}
    </>
  );
}
