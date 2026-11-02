import { Card } from "@minanode/design-system/components/card";
import { Skeleton } from "@minanode/design-system/components/skeleton";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { Signal } from "@minanode/design-system/patterns/signal";
import { Stat } from "@minanode/design-system/patterns/stat";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import { TextLink } from "@minanode/design-system/patterns/text-link";
import { formatAge, useNow } from "@minanode/design-system/lib/time";
import {
  formatCount,
  formatHeight,
  K,
  peerNote,
  plural,
  restoreHeadline,
  STATUS_POLL_MS,
  utcDateTime,
} from "@minanode/node-client";
import {
  useNodeStatus,
  usePoll,
  useSyncRestore,
  useSyncView,
  useVerification,
} from "@minanode/node-client/react";
import { PolledAge } from "minanode-dashboard/components/states";

import { useBoot } from "../app/boot-context";
import { Link } from "../app/router";
import { formatBytes } from "../boot/failures";
import { CONFIG } from "../config";
import { parseMemoryStats } from "../lib/memory";

const MEMORY_POLL_MS = 5_000;

export function OverviewPage() {
  const now = useNow();
  const boot = useBoot();
  const { status, build, peers } = useNodeStatus();
  const view = useSyncView(now);
  const s = status.data;
  const restore = useSyncRestore();
  const checks = useVerification(1, 10_000);
  const glue = boot.glue;
  const memory = usePoll(
    glue?.memory_stats ? "memory" : null,
    async () => parseMemoryStats(glue?.memory_stats?.()),
    MEMORY_POLL_MS,
  );
  const m = memory.data;
  const last = checks.data?.lastVerified ?? null;

  return (
    <>
      <PageHeader
        eyebrow={`${s?.network ?? CONFIG.network} web node`}
        title="Overview"
        description="This node runs in this tab. It follows the chain and checks each new block's proof itself."
        aside={<PolledAge polled={status} staleAfterMs={STATUS_POLL_MS * 4} />}
      />

      <Card className="gap-0 overflow-hidden py-0">
        <div className="flex flex-col gap-4 p-6 md:flex-row md:items-start md:justify-between">
          <div className="flex min-w-0 flex-col gap-2">
            <p className="font-mono text-label text-brand-text uppercase">
              Sync
            </p>
            {status.loading ? (
              <div role="status" className="flex flex-col gap-2">
                <span className="sr-only">Loading the node's status…</span>
                <Skeleton className="h-9 w-72" />
                <Skeleton className="h-5 w-96 max-w-full" />
              </div>
            ) : (
              <>
                <h2 className="text-title text-balance text-foreground">
                  {view.headline}
                </h2>
                <p className="text-pretty text-muted-foreground">
                  {view.detail}
                </p>
              </>
            )}
            {restore.data ? (
              <p className="mt-1 w-fit rounded-md bg-brand-subtle px-3 py-2 text-sm text-foreground">
                {restoreHeadline(restore.data)}, from this browser's store.
              </p>
            ) : null}
          </div>
          <Signal
            tone={view.tone}
            pulse={view.tone === "info"}
            className="text-sm"
          >
            {view.label}
          </Signal>
        </div>
        <dl className="grid grid-cols-2 border-t xl:grid-cols-4 [&>div:nth-child(-n+2)]:border-b [&>div:nth-child(odd)]:border-r xl:[&>div]:border-b-0 xl:[&>div:not(:last-child)]:border-r">
          <Stat
            variant="labelled"
            inList
            className="px-4 py-5 sm:px-6"
            label="Best tip"
            value={s?.bestTip ? formatHeight(s.bestTip.height) : "–"}
            detail={
              (view.key === "Bootstrap" || view.key === "Catchup") &&
              view.peerDelta !== null &&
              view.peerDelta > K
                ? "its starting point"
                : s?.bestTip?.timestampMs
                  ? `block ${formatAge(now - s.bestTip.timestampMs)} old`
                  : "–"
            }
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-5 sm:px-6"
            label="Highest peer tip"
            value={formatHeight(peers?.highestTip ?? null)}
            detail={s ? peerNote(view.peerDelta) : "–"}
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-5 sm:px-6"
            label="Peers"
            value={peers ? formatCount(peers.connected) : "–"}
            detail={
              peers
                ? `connected${peers.connecting ? `, ${formatCount(peers.connecting)} connecting` : ""}`
                : "–"
            }
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-5 sm:px-6"
            label="Memory"
            value={m ? formatBytes(m.liveBytes) : "–"}
            detail={
              m?.wasmBytes
                ? `in use, of ${formatBytes(m.wasmBytes)} reserved`
                : "in use"
            }
          />
        </dl>
      </Card>

      <div className="grid gap-6 lg:grid-cols-2">
        <KeyValueList
          title="Block checks"
          aside={
            <TextLink asChild arrow className="font-sans text-sm">
              <Link to="/blocks">Blocks</Link>
            </TextLink>
          }
          rows={[
            {
              label: "Proofs checked",
              value: formatCount(checks.data?.verifiedSinceStart ?? null),
              tone: "brand",
            },
            {
              label: "Checks failed",
              value: formatCount(checks.data?.failuresSinceStart ?? null),
            },
            {
              label: "Median proof check",
              value:
                checks.data?.medianProofTimeMs != null
                  ? `${Math.round(checks.data.medianProofTimeMs)} ms`
                  : "–",
            },
            {
              label: "Last proof checked",
              value: last ? (
                <span>
                  {formatHeight(last.height)},{" "}
                  <TimeAgo at={last.proof.checkedAtMs ?? last.receivedAtMs} />
                </span>
              ) : (
                "–"
              ),
            },
          ]}
          footer="Since the node started in this tab. It checks the proofs here, not on a server."
        />
        <KeyValueList
          title="This node"
          rows={[
            {
              label: "Network",
              value: s?.network ?? CONFIG.network,
              tone: "brand",
            },
            {
              label: "Chain ID",
              value: s?.chainId ? (
                <Hash
                  value={s.chainId}
                  label="chain ID"
                  className="sm:justify-end"
                />
              ) : (
                "–"
              ),
            },
            { label: "Version", value: build.data?.version ?? "–" },
            {
              label: "Commit",
              value: build.data?.commit ? (
                <Hash
                  value={build.data.commit}
                  head={10}
                  tail={0}
                  label="commit"
                  className="sm:justify-end"
                />
              ) : (
                "–"
              ),
            },
            {
              label: "Built",
              value: build.data?.buildTime
                ? utcDateTime(Date.parse(build.data.buildTime))
                : "–",
            },
            {
              label: "Running for",
              value:
                s?.uptimeSecs != null ? formatAge(s.uptimeSecs * 1000) : "–",
            },
            {
              label: "Root",
              value: s?.root
                ? `${formatHeight(s.root.height)}${s.bestTip ? `, ${plural(s.bestTip.height - s.root.height, "block")} below the tip` : ""}`
                : "–",
            },
            {
              label: "Frontier store",
              value: boot.frontierStore ? "On" : "Off",
            },
          ]}
          footer={
            boot.frontierStore
              ? "The store keeps the node's ledgers and blocks in this browser, so a reload starts from them. Start-up has a reset that deletes it."
              : "Without the store, a reload syncs from the network again."
          }
        />
      </div>
    </>
  );
}
