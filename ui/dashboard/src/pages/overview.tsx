import * as React from "react";
import { Box, Cpu, History, TriangleAlert } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Card } from "@minanode/design-system/components/card";
import { Skeleton } from "@minanode/design-system/components/skeleton";
import { Measure } from "@minanode/design-system/patterns/amount";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { RoleCard } from "@minanode/design-system/patterns/role-card";
import { Signal, SignalDot } from "@minanode/design-system/patterns/signal";
import { Stat } from "@minanode/design-system/patterns/stat";
import { RelativeTime } from "@minanode/design-system/patterns/relative-time";
import { TextLink } from "@minanode/design-system/patterns/text-link";
import { formatAge, useNow } from "@minanode/design-system/lib/time";
import { cn } from "@minanode/design-system/lib/utils";

import {
  formatCount,
  formatHeight,
  formatMina,
  K,
  kindOf,
  peerNote,
  plural,
  restoreHeadline,
  STATUS_POLL_MS,
  utcDateTime,
  utcTime,
  workerView,
  type RestoreSummary,
} from "@minanode/node-client";
import {
  useCrashNotice,
  useNodeStatus,
  useSyncRestore,
  useSyncView,
  useWorkerControl,
} from "@minanode/node-client/react";

import { Link } from "../app/router";
import { Count } from "../components/cells";
import { PolledAge } from "../components/states";
import { docsUrl } from "../lib/docs";

export function OverviewPage() {
  const now = useNow();
  const { status, build, peers } = useNodeStatus();
  const view = useSyncView(now);
  const s = status.data;
  const restore = useSyncRestore();

  return (
    <>
      <PageHeader
        eyebrow="Node"
        title="Overview"
        aside={
          <>
            <PolledAge polled={status} staleAfterMs={STATUS_POLL_MS * 4} />
            {/* The one link to the explanation. */}
            <ExternalLink href={docsUrl("overview")} className="text-sm">
              Dashboard docs
            </ExternalLink>
          </>
        }
      />

      <CrashNotice />

      <Card className="overflow-hidden">
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
                id="sync-heading"
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
          {restore.data && restoreIsNews(restore.data, now) ? (
            <RestoreLine restore={restore.data} />
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
                  : s
                    ? "no block time reported"
                    : "–"
            }
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
            label="Highest peer tip"
            value={formatHeight(peers?.highestTip ?? null)}
            detail={s ? peerNote(view.peerDelta) : "–"}
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
            label="Peers"
            value={peers ? formatCount(peers.connected) : "–"}
            detail={
              peers
                ? `connected${peers.connecting ? `, ${formatCount(peers.connecting)} connecting` : ""}, ${formatCount(peers.disconnected)}\u00a0more\u00a0known`
                : "–"
            }
          />
          <Stat
            variant="labelled"
            inList
            className="px-4 py-4 sm:py-5"
            label="Root"
            value={s?.root ? formatHeight(s.root.height) : "–"}
            detail={
              s?.root && s.bestTip
                ? s.bestTip.height > s.root.height
                  ? `${plural(s.bestTip.height - s.root.height, "block")} below the tip`
                  : undefined
                : s
                  ? "the oldest block it keeps"
                  : "–"
            }
          />
        </dl>
      </Card>

      <PageSection title="Roles">
        <div className="grid gap-6 lg:grid-cols-2">
          <ProducerRole />
          <WorkerRole />
        </div>
      </PageSection>

      <div className="grid items-start gap-6 lg:grid-cols-2">
        <KeyValueList
          title="This node"
          rows={[
            { label: "Network", value: s?.network ?? "–", tone: "brand" },
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
            ...(restore.data && !restoreIsNews(restore.data, now)
              ? [
                  {
                    label: "Started from",
                    value: (
                      <span>
                        Saved state at{" "}
                        <span className="font-mono text-[0.8125rem] tabular-nums">
                          {formatHeight(restore.data.tipHeight)}
                        </span>
                      </span>
                    ),
                    prose: true,
                  },
                ]
              : []),
            {
              label: "Uptime",
              value: (
                <Measure
                  value={
                    s?.uptimeSecs !== null && s?.uptimeSecs !== undefined
                      ? formatAge(s.uptimeSecs * 1000)
                      : null
                  }
                />
              ),
            },
          ]}
        />
        <KeyValueList
          title="Pools"
          aside={
            <TextLink asChild arrow className="font-sans text-sm">
              <Link to="/mempool">Mempool</Link>
            </TextLink>
          }
          rows={[
            {
              label: "Pending transactions",
              value: <Count n={s?.transactionPool.transactions ?? null} />,
              tone: "brand",
            },
            {
              label: "SNARK jobs",
              value: <Count n={s?.snarkPool.jobs ?? null} />,
            },
            {
              label: "SNARK jobs with a proof",
              value: <Count n={s?.snarkPool.snarks ?? null} />,
            },
          ]}
        />
      </div>
    </>
  );
}

/** "The node crashed at ...", when it panicked in this run or just before
 * its current start, with a way to report it. */
export function CrashNotice() {
  const notice = useCrashNotice();
  const crash = notice.data;
  if (!crash) return null;
  return (
    <Alert variant="destructive">
      <TriangleAlert aria-hidden />
      <AlertTitle>
        The node crashed at{" "}
        {/* A time as every absolute time is set, kept whole. */}
        <span className="whitespace-nowrap">
          <Measure value={utcDateTime(crash.crashedMs)} />
        </span>
      </AlertTitle>
      <AlertDescription>
        <p>
          {crash.thisRun
            ? "A thread panicked while it ran"
            : "It restarted after the panic"}
          {crash.message ? (
            <>
              :{" "}
              <code className="font-mono break-words text-foreground">
                {crash.message}
              </code>
            </>
          ) : null}
          .
        </p>
        <TextLink asChild arrow className="w-fit font-medium">
          <Link to="/report-a-problem">Report a problem</Link>
        </TextLink>
      </AlertDescription>
    </Alert>
  );
}

/** How long a finished restore stays in the sync card: the start's news.
 * After that, This node says where it started from. */
const RESTORE_NEWS_MS = 10 * 60_000;

function restoreIsNews(r: RestoreSummary, now: number): boolean {
  return r.failed || !r.done || r.endedAt === null
    ? true
    : now - r.endedAt < RESTORE_NEWS_MS;
}

function RestoreLine({ restore }: { restore: RestoreSummary }) {
  const Icon = restore.failed ? TriangleAlert : History;
  return (
    <p
      className={cn(
        // Inside the sync card: a tint, no border of its own.
        "mt-1 inline-flex w-fit items-start gap-2 rounded-md px-3 py-2 text-sm text-foreground",
        restore.failed ? "bg-status-pending/8" : "bg-brand-subtle",
      )}
    >
      <Icon
        aria-hidden
        className={cn(
          "mt-0.5 size-4 shrink-0",
          restore.failed ? "text-status-pending" : "text-brand-text",
        )}
      />
      <span>
        {restoreHeadline(restore)}
        {restore.failed && restore.failure ? (
          <span className="block text-muted-foreground">{restore.failure}</span>
        ) : null}
      </span>
    </p>
  );
}

/** A link to a role's page, at the bottom of its card. */
function RoleLink({ to, children }: { to: string; children: React.ReactNode }) {
  return (
    <TextLink asChild arrow className="text-sm font-medium">
      <Link to={to}>{children}</Link>
    </TextLink>
  );
}

function ProducerRole() {
  const { status } = useNodeStatus();
  const s = status.data;
  const key = s?.blockProducer ?? null;
  // No answer yet: say so rather than "doesn't produce".
  const unknown = !s && status.error !== null;
  return (
    <RoleCard
      icon={<Box />}
      title="Block producer"
      action={
        <RoleLink to="/block-producer">
          {key ? "Production details" : "How to turn it on"}
        </RoleLink>
      }
      status={
        s ? (
          <Signal tone={key ? "ok" : "neutral"}>{key ? "On" : "Off"}</Signal>
        ) : unknown ? (
          <Signal tone="neutral">Unknown</Signal>
        ) : (
          <Skeleton className="h-6 w-14 rounded-full" />
        )
      }
    >
      {key ? (
        <>
          <span className="flex flex-wrap items-center gap-1">
            Producing for <Hash value={key} label="producer key" />
          </span>
          <span>
            {s?.nextWonSlot?.startTimeMs ? (
              <>
                Next won slot <RelativeTime at={s.nextWonSlot.startTimeMs} />,
                at {/* As every absolute time: mono, UTC muted. */}
                <Measure
                  value={`${utcTime(s.nextWonSlot.startTimeMs).slice(0, 5)} UTC`}
                />
                .
              </>
            ) : s?.nextWonSlot ? (
              `Next won slot: epoch ${s.nextWonSlot.epoch}, slot ${s.nextWonSlot.slot}.`
            ) : (
              "No won slot found yet in the epochs evaluated."
            )}
          </span>
        </>
      ) : unknown ? (
        <span>Not known: the node isn't answering.</span>
      ) : s ? (
        <span>This node doesn't produce blocks.</span>
      ) : (
        <span>Loading…</span>
      )}
    </RoleCard>
  );
}

function WorkerRole() {
  const control = useWorkerControl(10_000);
  const c = control.data;
  const view = c ? workerView(c) : null;
  return (
    <RoleCard
      icon={<Cpu />}
      title="SNARK worker"
      action={
        <RoleLink to="/snark-worker">
          {c?.allowed ? "Manage the worker" : "Worker details"}
        </RoleLink>
      }
      status={
        view ? (
          <Signal tone={view.tone} pulse={view.label === "Working"}>
            {view.label}
          </Signal>
        ) : control.error ? (
          <Signal tone="neutral">Unknown</Signal>
        ) : (
          <Skeleton className="h-6 w-16 rounded-full" />
        )
      }
    >
      {c ? (
        <>
          <span>{view?.detail}</span>
          {c.publicKey ? (
            <span className="flex flex-wrap items-center gap-1">
              Fees of {formatMina(c.fee) ?? "–"} MINA a job to
              <Hash value={c.publicKey} label="fee receiver key" />
            </span>
          ) : null}
          {c.proofsMade ? (
            <span>
              {plural(c.proofsMade, "proof")} made since the node started.
            </span>
          ) : null}
        </>
      ) : control.error ? (
        <span>
          {kindOf(control.error) === "offline"
            ? "Not known: the node isn't answering."
            : "This node doesn't report its worker."}
        </span>
      ) : (
        <span>Loading…</span>
      )}
    </RoleCard>
  );
}
