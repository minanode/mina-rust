import { Card } from "@minanode/design-system/components/card";
import { Skeleton } from "@minanode/design-system/components/skeleton";
import { Measure } from "@minanode/design-system/patterns/amount";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { SignalDot } from "@minanode/design-system/patterns/signal";
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
import {
  savedChainHeight,
  savedChainLabel,
  storeLabel,
  storeState,
} from "../lib/store";

const MEMORY_POLL_MS = 5_000;

/** A count, muted at zero, as zeros are in the tables. */
function Muted0({ n }: { n: number | null }) {
  return (
    <span className={n === 0 ? "text-muted-foreground" : undefined}>
      {formatCount(n)}
    </span>
  );
}

export function OverviewPage() {
  const now = useNow();
  const boot = useBoot();
  const { status, build, peers } = useNodeStatus();
  const view = useSyncView(now);
  const stale = view.key === "NoAnswer";
  const s = status.data;
  const store = storeState(boot.frontierStore, s?.frontierStore);
  const saved = savedChainLabel(store, s?.frontierStore?.savedChain);
  const savedHeight = savedChainHeight(store, s?.frontierStore?.savedChain);
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
        eyebrow="Node"
        title="Overview"
        aside={<PolledAge polled={status} staleAfterMs={STATUS_POLL_MS * 4} />}
      />

      <Card className="gap-0 overflow-hidden py-0">
        <div className="flex min-w-0 flex-col gap-1.5 px-4 py-4">
          {status.loading ? (
            <div role="status" className="flex flex-col gap-2">
              <span className="sr-only">Loading the node's status…</span>
              <Skeleton className="h-6 w-56" />
              <Skeleton className="h-5 w-96 max-w-full" />
            </div>
          ) : (
            <>
              <h2
                className="flex items-center gap-2.5 text-heading text-balance text-foreground"
                data-sync={view.key}
              >
                <SignalDot
                  tone={view.tone}
                  pulse={view.tone === "info"}
                  className="size-2.5 [&>span]:size-2.5"
                />
                {/* The best tip has the height; the header's word. */}
                {view.key === "Synced" ? view.label : view.headline}
              </h2>
              {/* In sync, the figures below say what the sentence would. */}
              {view.key === "Synced" && view.tone === "ok" ? null : (
                <p className="text-sm text-pretty text-muted-foreground">
                  {view.detail}
                </p>
              )}
            </>
          )}
          {/* Old news while the node doesn't answer: hidden, so the hero
              says one thing. */}
          {restore.data && view.key !== "NoAnswer" ? (
            <p className="mt-1.5 w-fit rounded-md bg-brand-subtle px-3 py-2 text-sm text-foreground">
              {restoreHeadline(restore.data)}, from this browser's store.
            </p>
          ) : null}
        </div>
        <dl className="grid grid-cols-2 border-t xl:grid-cols-4 max-xl:[&>div:nth-child(-n+2)]:border-b max-xl:[&>div:nth-child(odd)]:border-r xl:[&>div:not(:last-child)]:border-r">
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
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
            className="px-4 py-4 sm:py-5"
            label="Highest peer tip"
            value={formatHeight(peers?.highestTip ?? null)}
            // With no answer the figures are the last ones: said so.
            detail={
              stale ? "at the last answer" : s ? peerNote(view.peerDelta) : "–"
            }
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
            label="Peers"
            value={peers ? formatCount(peers.connected) : "–"}
            detail={
              stale
                ? "connected at the last answer"
                : peers
                  ? `connected${peers.connecting ? `, ${formatCount(peers.connecting)} connecting` : ""}`
                  : "–"
            }
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
            label="Memory"
            value={
              m ? (
                <Measure
                  value={formatBytes(m.liveBytes)}
                  unitClassName="font-sans text-base font-normal"
                />
              ) : (
                "–"
              )
            }
            detail={
              m?.wasmBytes ? `${formatBytes(m.wasmBytes)} reserved` : "in use"
            }
          />
        </dl>
      </Card>

      <div className="grid items-start gap-6 lg:grid-cols-2">
        <div className="flex flex-col gap-6">
          <KeyValueList
            title={
              <>
                Block checks{" "}
                <span className="font-normal text-muted-foreground">
                  since start
                </span>
              </>
            }
            aside={
              <TextLink asChild arrow className="font-sans text-sm">
                <Link to="/blocks">Blocks</Link>
              </TextLink>
            }
            rows={[
              {
                label: "Proofs checked",
                value: formatCount(checks.data?.verifiedSinceStart ?? null),
              },
              {
                label: "Checks failed",
                value: <Muted0 n={checks.data?.failuresSinceStart ?? null} />,
              },
              {
                label: "Median proof check",
                value:
                  checks.data?.medianProofTimeMs != null ? (
                    <Measure
                      value={`${Math.round(checks.data.medianProofTimeMs)} ms`}
                    />
                  ) : (
                    "–"
                  ),
              },
              {
                label: "Last proof checked",
                value: last ? (
                  <span className="whitespace-nowrap">
                    {formatHeight(last.height)}
                    <TimeAgo
                      at={last.proof.checkedAtMs ?? last.receivedAtMs}
                      className="ml-2 text-muted-foreground"
                    />
                  </span>
                ) : (
                  "–"
                ),
              },
            ]}
          />
          <KeyValueList
            title="Frontier store"
            rows={[
              {
                label: "Status",
                value: (
                  <span data-frontier-store-status>{storeLabel(store)}</span>
                ),
                prose: true,
              },
              ...(saved
                ? [
                    savedHeight !== null
                      ? {
                          label: "Saved chain",
                          value: (
                            <span data-saved-chain>
                              {formatHeight(savedHeight)}
                            </span>
                          ),
                        }
                      : {
                          label: "Saved chain",
                          value: <span data-saved-chain>{saved}</span>,
                          prose: true,
                        },
                  ]
                : []),
            ]}
          />
        </div>
        <KeyValueList
          title="This node"
          rows={[
            {
              label: "Network",
              value: s?.network ?? CONFIG.network,
              prose: true,
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
              value: (
                <Measure
                  value={
                    build.data?.buildTime
                      ? utcDateTime(Date.parse(build.data.buildTime))
                      : null
                  }
                />
              ),
            },
            {
              label: "Running for",
              note: stale ? "at the last answer" : undefined,
              value:
                s?.uptimeSecs != null ? (
                  <Measure value={formatAge(s.uptimeSecs * 1000)} />
                ) : (
                  "–"
                ),
            },
            {
              label: "Root",
              value: s?.root ? (
                <>
                  {formatHeight(s.root.height)}
                  {s.bestTip ? (
                    <span className="ml-2 font-sans text-muted-foreground">
                      {plural(s.bestTip.height - s.root.height, "block")} below
                      the tip
                    </span>
                  ) : null}
                </>
              ) : (
                "–"
              ),
            },
          ]}
        />
      </div>
    </>
  );
}
