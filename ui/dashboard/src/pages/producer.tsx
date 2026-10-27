import * as React from "react";
import { Box, ChevronDown } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
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
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import {
  RelativeTime,
  TimeUntil,
} from "@minanode/design-system/patterns/relative-time";
import { SegmentedControl } from "@minanode/design-system/patterns/segmented-control";
import { Signal, toneFill } from "@minanode/design-system/patterns/signal";
import { StageBar } from "@minanode/design-system/patterns/stage-bar";
import { StatRow } from "@minanode/design-system/patterns/stat-row";
import { TextLink } from "@minanode/design-system/patterns/text-link";
import {
  Timeline,
  type TimelineStep,
} from "@minanode/design-system/patterns/timeline";
import {
  attemptReports,
  epochExpectations,
  explorerBlockUrl,
  formatCount,
  formatDuration,
  formatHeight,
  formatMina,
  formatOffset,
  plural,
  productionSummary,
  reportsStake,
  slotDurationMs,
  startedAt,
  upcomingWonSlots,
  utcDateTime,
  utcTime,
  workerView,
  type AttemptReport,
  type EpochExpectation,
  type ProducerStats,
  type UpcomingSlot,
} from "@minanode/node-client";
import {
  useNodeStatus,
  useProducerStats,
  useWorkerControl,
} from "@minanode/node-client/react";

import { Link } from "../app/router";
import { Code } from "../components/code";
import { useShowAll } from "../components/show-all";
import { Loaded, PolledAge } from "../components/states";
import { docsUrl } from "../lib/docs";

const POLL_MS = 10_000;

/** A slot-sized time in whole seconds: `90 s`, not `1 min 30 s`. */
const seconds = (ms: number) => `${Math.round(ms / 1000)} s`;

export function ProducerPage() {
  const { status } = useNodeStatus();
  const key = status.data?.blockProducer ?? null;
  const stats = useProducerStats(POLL_MS);
  const on = key !== null || stats.data !== null;

  return (
    <>
      <PageHeader
        eyebrow="Roles"
        title="Block producer"
        description={
          <>
            Whether this node produces blocks, the slots it won, and what became
            of each: its stages against the slot, the outcome, and the totals.
            Read-only.{" "}
            <ExternalLink href={docsUrl("producer")}>
              Block producer docs
            </ExternalLink>
          </>
        }
        aside={
          <>
            {status.data || stats.data ? (
              <Signal tone={on ? "ok" : "neutral"} className="text-sm">
                {on ? "On" : "Off"}
              </Signal>
            ) : null}
            <PolledAge polled={stats} staleAfterMs={POLL_MS * 4} />
          </>
        }
      />
      <Loaded polled={stats} what="block production" rows={4}>
        {data =>
          data === null ? (
            <EmptyState
              icon={<Box />}
              title="Block production is off"
              description={
                <>
                  <p>
                    This node follows the chain without producing blocks. To
                    produce, restart it with{" "}
                    <Code>--producer-key &lt;file&gt;</Code> and the key's
                    password in <Code>MINA_PRIVKEY_PASS</Code>.
                  </p>
                </>
              }
            />
          ) : (
            <Producing stats={data} />
          )
        }
      </Loaded>
    </>
  );
}

function Producing({ stats }: { stats: ProducerStats }) {
  const { status } = useNodeStatus();
  const uptime = status.data?.uptimeSecs ?? null;
  const network = status.data?.network ?? null;
  const reports = React.useMemo(
    () => attemptReports(stats, uptime),
    [stats, uptime],
  );
  const startedMs = startedAt(stats, uptime);
  // The scheduled attempt's slot first: it left futureWonSlots. One the run
  // before a restart scheduled isn't this run's.
  const ahead = upcomingWonSlots(stats, startedMs);
  const slotMs = slotDurationMs(stats);
  return (
    <>
      <Facts stats={stats} ahead={ahead} />
      <Summary
        stats={stats}
        reports={reports}
        ahead={ahead}
        slotMs={slotMs}
        restartedMs={startedMs}
      />
      <Expectations stats={stats} />
      <SlotsAhead ahead={ahead} />
      <Attempts reports={reports} network={network} slotMs={slotMs} />
    </>
  );
}

function Facts({
  stats,
  ahead,
}: {
  stats: ProducerStats;
  ahead: UpcomingSlot[];
}) {
  const { status } = useNodeStatus();
  const worker = useWorkerControl();
  const s = status.data;
  const w = worker.data;
  const next = ahead[0] ?? null;
  const vrf = stats.vrf;
  const workerOn = w?.enabled === true;
  const view = w ? workerView(w) : null;
  return (
    <div className="grid gap-6 lg:grid-cols-2">
      <KeyValueList
        title="Producer"
        rows={[
          {
            label: "Block producer key",
            value: stats.publicKey ? (
              <Hash
                value={stats.publicKey}
                label="producer key"
                className="sm:justify-end"
              />
            ) : (
              "–"
            ),
          },
          {
            label: "Coinbase receiver",
            value: s?.coinbaseReceiver ? (
              <Hash
                value={s.coinbaseReceiver}
                label="coinbase receiver"
                className="sm:justify-end"
              />
            ) : (
              "The producer key"
            ),
          },
          {
            label: "Next won slot",
            value: next ? (
              <span>
                <TimeUntil at={next.slotTimeMs} /> (
                {next.slotTimeMs !== null &&
                next.slotTimeMs - Date.now() < 86_400_000
                  ? utcTime(next.slotTimeMs).slice(0, 5) + " UTC"
                  : utcDateTime(next.slotTimeMs)}
                ){next.scheduled ? ", scheduled" : null}
              </span>
            ) : s?.nextWonSlot ? (
              `epoch ${s.nextWonSlot.epoch}, slot ${s.nextWonSlot.slot}`
            ) : (
              "None found yet"
            ),
            tone: "brand",
          },
          ...(workerOn && view
            ? [
                {
                  label: "SNARK worker",
                  value: (
                    <span className="inline-flex flex-wrap items-center gap-x-3 gap-y-1 sm:justify-end">
                      <Signal tone={view.tone}>{view.label}</Signal>
                      <TextLink asChild arrow className="font-sans text-sm">
                        <Link to="/snark-worker">Worker</Link>
                      </TextLink>
                    </span>
                  ),
                },
              ]
            : []),
        ]}
        footer={
          workerOn ? (
            <>
              The worker on this node takes no new job in the minutes before a
              won slot (<Code>--snarker-block-first-lead</Code>, 7 min by
              default), so the block has the machine to itself.
            </>
          ) : undefined
        }
      />
      <KeyValueList
        title="This epoch"
        rows={[
          { label: "Epoch", value: formatCount(stats.currentEpoch) },
          {
            label: "Global slot now",
            value: formatCount(stats.currentGlobalSlot),
          },
          {
            label: "Epoch slots",
            value:
              stats.epochStart !== null && stats.epochEnd !== null
                ? `${formatCount(stats.epochStart)} to ${formatCount(stats.epochEnd)}`
                : "–",
          },
          {
            label: "Slots checked for wins",
            value: vrf ? (
              <span className="inline-flex flex-col items-end gap-1.5">
                <span>
                  {formatCount(vrf.evaluated)} of {formatCount(vrf.total)}
                </span>
                <progress
                  max={vrf.total || 1}
                  value={vrf.evaluated}
                  aria-label="Slots checked for wins"
                  className="h-1.5 w-32 overflow-hidden rounded-full bg-muted [&::-moz-progress-bar]:bg-primary [&::-webkit-progress-bar]:bg-muted [&::-webkit-progress-value]:bg-primary"
                />
              </span>
            ) : (
              "–"
            ),
          },
        ]}
      />
    </div>
  );
}

type Scope = "epoch" | "all";

function Summary({
  stats,
  reports,
  ahead,
  slotMs,
  restartedMs,
}: {
  stats: ProducerStats;
  reports: AttemptReport[];
  ahead: UpcomingSlot[];
  slotMs: number;
  restartedMs: number | null;
}) {
  const [scope, setScope] = React.useState<Scope>("epoch");
  const epoch = stats.currentEpoch;
  const inScope =
    scope === "epoch"
      ? reports.filter(r => r.attempt.wonSlot.epoch === epoch)
      : reports;
  const aheadInScope =
    scope === "epoch" ? ahead.filter(w => w.epoch === epoch) : ahead;
  const sum = productionSummary(inScope, aheadInScope);
  const kept = sum.sinceMs !== null;
  const beforeRestart =
    restartedMs !== null &&
    sum.sinceMs !== null &&
    sum.sinceMs < restartedMs - slotMs;
  const sent = sum.produced;
  return (
    <PageSection
      title="Summary"
      description={
        scope === "epoch"
          ? `Epoch ${formatCount(epoch)}: the won slots that have come, what became of them, and how long their blocks took.`
          : kept
            ? `Every won slot the node keeps, from ${utcDateTime(sum.sinceMs)}${beforeRestart ? ", from before its latest restart" : ""}.`
            : "Every won slot the node keeps."
      }
      aside={
        <SegmentedControl
          label="Which slots"
          options={[
            { value: "epoch", label: "This epoch" },
            { value: "all", label: "Since start" },
          ]}
          value={scope}
          onValueChange={setScope}
        />
      }
    >
      <StatRow
        label="Outcomes"
        items={[
          {
            label: "Won",
            value: formatCount(sum.won),
            detail:
              sum.producing > 0
                ? `${formatCount(sum.ahead)} ahead, ${formatCount(sum.producing)} under way`
                : `${formatCount(sum.ahead)} ahead`,
          },
          {
            label: "Produced",
            value: formatCount(sent),
            detail: "built, proved and sent",
          },
          {
            label: "In the chain",
            value: formatCount(sum.canonical),
            detail:
              sent > 0
                ? `${Math.round((sum.canonical / sent) * 100)}% of those sent`
                : "in the best chain",
          },
          {
            label: "Orphaned",
            value: formatCount(sum.orphaned),
            detail: "sent, then replaced",
          },
          {
            label: "Lost the tie",
            value: formatCount(sum.lostTie),
            detail: "another block for the slot won",
          },
          {
            label: "Missed",
            value: formatCount(sum.missed),
            detail: "the slot passed without ours",
          },
        ]}
      />
      <StatRow
        label="Times and coinbase"
        minItemWidth="12rem"
        items={[
          {
            label: "Median proof",
            value: sum.proof ? formatDuration(sum.proof.median) : "–",
            detail: sum.proof
              ? `over ${plural(sum.proof.count, "proof")}`
              : "no proof finished",
          },
          {
            label: "Worst proof",
            value: sum.proof ? formatDuration(sum.proof.worst) : "–",
            detail: `in a ${seconds(slotMs)} slot`,
          },
          {
            label: "Median broadcast",
            value: sum.broadcast ? formatOffset(sum.broadcast.median) : "–",
            detail: sum.broadcast
              ? `from the slot's start; latest ${formatOffset(sum.broadcast.worst)}`
              : "from the slot's start",
          },
          {
            label: "Coinbase earned",
            value: <Amount unit="MINA" value={formatMina(sum.coinbase)} />,
            detail:
              sum.canonical > 0
                ? `to the coinbase receiver, from ${plural(sum.canonical, "block")} in the chain; fees +${formatMina(sum.fees)}, SNARK work −${formatMina(sum.snarkFees)} MINA`
                : "to the coinbase receiver, from blocks in the chain",
          },
        ]}
      />
    </PageSection>
  );
}

/** Where the count fell against the 95% range, and, with few wins
 * expected, that it says little either way. */
function verdict(e: EpochExpectation): string {
  const place =
    e.found < e.low
      ? "below the range"
      : e.found > e.high
        ? "above the range"
        : "within the range";
  return e.expected < 5 ? `${place}; too few wins expected to judge` : place;
}

function Expectations({ stats }: { stats: ProducerStats }) {
  const rows = epochExpectations(stats.vrfEpochs);
  return (
    <PageSection
      title="Expected against found"
      description="How many slots this key should win for its stake, against how many the node found. Each slot is a draw, so the count varies by chance: it lands in the 95% range 95 times in 100. A count outside it is unusual, not proof of a fault; with few wins expected, the range says little."
    >
      {!reportsStake(stats.vrfEpochs) || rows.length === 0 ? (
        <EmptyState
          title="Not reported by this node"
          description={
            <p>
              This node's build doesn't send the stake behind its search for won
              slots (<Code>vrf_stats</Code> has no <Code>delegated_stake</Code>
              ), so the wins to expect can't be worked out here. Newer builds
              send it.
            </p>
          }
        />
      ) : (
        <>
          <Table
            className="[&_td]:align-top"
            containerProps={tableRegion("Expected against found")}
          >
            <TableHeader>
              <TableRow>
                <TableHead className="text-right">Epoch</TableHead>
                <TableHead className="text-right">Stake</TableHead>
                <TableHead className="text-right">Slots checked</TableHead>
                <TableHead className="text-right">Expected</TableHead>
                <TableHead className="text-right">Found</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows
                .slice()
                .reverse()
                .map(e => (
                  <TableRow key={e.epoch}>
                    <TableCell className="text-right font-mono tabular-nums">
                      {formatCount(e.epoch)}
                    </TableCell>
                    <TableCell className="text-right">
                      {/* Whole MINA: a stake's nanomina pass 2^53, past
                        exact JSON numbers, but whole MINA stay exact. */}
                      <Amount
                        unit="MINA"
                        value={formatCount(
                          Math.round(e.delegatedStake / 1_000_000_000),
                        )}
                      />
                      <span className="block pt-1 text-xs text-muted-foreground">
                        {(e.share * 100).toFixed(2)}% of the stake
                        {e.delegators !== null
                          ? `, delegated by ${plural(e.delegators, "account")}`
                          : ""}
                      </span>
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      {formatCount(e.slotsChecked)}
                      {e.slotsChecked < e.totalSlots ? (
                        <span className="block pt-1 font-sans text-xs text-muted-foreground">
                          of {formatCount(e.totalSlots)}
                        </span>
                      ) : null}
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      {e.expected.toFixed(1)}
                      <span className="block pt-1 font-sans text-xs text-muted-foreground">
                        95% range: {formatCount(e.low)} to {formatCount(e.high)}
                      </span>
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      {formatCount(e.found)}
                      <span className="block pt-1 font-sans text-xs text-muted-foreground">
                        {verdict(e)}
                      </span>
                    </TableCell>
                  </TableRow>
                ))}
            </TableBody>
          </Table>
          <p className="text-xs text-muted-foreground">
            A slot is won with the chance 1 − (1/4)^s, s being the key's share
            of the epoch's staking ledger (Mina's active slot coefficient, 3/4);
            the wins to expect are that times the slots checked, and the range
            comes from the binomial distribution.
          </p>
        </>
      )}
    </PageSection>
  );
}

function SlotsAhead({ ahead }: { ahead: UpcomingSlot[] }) {
  const { shown, hidden, toggle } = useShowAll(ahead, "soonest");
  return (
    <PageSection
      title="Won slots ahead"
      description="Slots this node won and will produce in, soonest first. Scheduled: the node has its attempt ready and waits for the slot."
      aside={toggle}
    >
      {ahead.length === 0 ? (
        <EmptyState
          title="No won slot ahead"
          description={<p>None in the slots checked so far.</p>}
        />
      ) : (
        <Table
          className="[&_td]:align-middle [&_td]:whitespace-nowrap"
          containerProps={tableRegion("Won slots ahead")}
        >
          <TableHeader>
            <TableRow>
              <TableHead>Slot time</TableHead>
              <TableHead>Starts in</TableHead>
              <TableHead className="text-right">Global slot</TableHead>
              <TableHead className="text-right">Epoch</TableHead>
              <TableHead>
                <span className="sr-only">Status</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {shown.map(w => (
              <TableRow key={w.globalSlot ?? w.slotTimeMs}>
                <TableCell className="font-mono">
                  {utcDateTime(w.slotTimeMs)}
                </TableCell>
                <TableCell>
                  <TimeUntil at={w.slotTimeMs} />
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {formatCount(w.globalSlot)}
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {formatCount(w.epoch)}
                </TableCell>
                <TableCell>
                  {w.scheduled ? <Signal tone="info">Scheduled</Signal> : null}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {hidden > 0 ? (
        <p className="text-sm text-muted-foreground">
          The first {shown.length} of {formatCount(ahead.length)}; the last on{" "}
          {utcDateTime(ahead[ahead.length - 1].slotTimeMs)}.
        </p>
      ) : null}
    </PageSection>
  );
}

/** The bars' common span: from just before the earliest stage to the end of
 * the slot, so the rows line up. */
function barDomain(reports: AttemptReport[], slotMs: number): [number, number] {
  let from = -10_000;
  for (const r of reports) {
    const first =
      r.attempt.times.stagedLedgerDiffStart ?? r.attempt.times.produced;
    if (first !== null && r.slotStartMs !== null) {
      // A stage minutes early (a slow diff) would squash the slot: cap it.
      from = Math.min(from, Math.max(first - r.slotStartMs - 2_000, -slotMs));
    }
  }
  return [from, slotMs];
}

function Legend() {
  const item = (fill: string, text: string, round = false) => (
    <span className="inline-flex items-center gap-1.5">
      <span
        aria-hidden
        className={`inline-block ${round ? "size-2.5 rounded-full" : "h-1.5 w-4 rounded-full"} ${fill}`}
      />
      {text}
    </span>
  );
  return (
    <p className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
      {item("bg-muted-foreground/25", "the slot")}
      {item(toneFill.neutral, "building")}
      {item(toneFill.info, "proving")}
      {item(toneFill.ok, "sent, in the chain", true)}
      {item(toneFill.warn, "dropped or replaced", true)}
    </p>
  );
}

function Attempts({
  reports,
  network,
  slotMs,
}: {
  reports: AttemptReport[];
  network: string | null;
  slotMs: number;
}) {
  const domain = barDomain(reports, slotMs);
  const { shown, toggle } = useShowAll(reports, "newest");
  return (
    <PageSection
      title="Won slots"
      description={`Newest first: what became of each, and when each stage happened against its ${seconds(slotMs)} slot, from the slot's start.`}
      aside={toggle}
    >
      {reports.length === 0 ? (
        <EmptyState
          title="No won slot has come yet"
          description={<p>Each appears here once its slot comes up.</p>}
        />
      ) : (
        <>
          <Legend />
          <ul className="flex flex-col divide-y rounded-xl border bg-card">
            {shown.map(r => (
              <AttemptRow
                // By slot and when it was scheduled, not by place: a new
                // attempt on top keeps an open row open.
                key={`${r.attempt.wonSlot.globalSlot}-${r.attempt.times.scheduled}`}
                report={r}
                network={network}
                domain={domain}
                slotMs={slotMs}
              />
            ))}
          </ul>
        </>
      )}
    </PageSection>
  );
}

function barLabel(r: AttemptReport, slotMs: number): string {
  const slot = formatCount(r.attempt.wonSlot.globalSlot);
  const parts = [`Slot ${slot}, ${seconds(slotMs)} long`];
  const built = r.stages.find(s => s.key === "built");
  if (built?.offsetMs != null)
    parts.push(`built at ${formatOffset(built.offsetMs)}`);
  if (r.proofMs !== null) parts.push(`proof ${formatDuration(r.proofMs)}`);
  if (r.broadcastOffsetMs !== null)
    parts.push(`sent at ${formatOffset(r.broadcastOffsetMs)}`);
  else if (r.discardedOffsetMs !== null)
    parts.push(`dropped at ${formatOffset(r.discardedOffsetMs)}`);
  return `${parts.join(", ")}.`;
}

function Caption({ report: r }: { report: AttemptReport }) {
  if (r.outcome === "upcoming") {
    return (
      <>
        Starts <TimeUntil at={r.slotStartMs} />
      </>
    );
  }
  if (r.outcome === "producing") return <>Under way</>;
  if (r.outcome === "previousRun") return <>From before the restart</>;
  if (r.interrupted) return <>Interrupted by a restart</>;
  if (r.broadcastOffsetMs !== null) {
    return (
      <>
        Out at {formatOffset(r.broadcastOffsetMs)}
        {r.proofMs !== null ? ` · proof ${formatDuration(r.proofMs)}` : null}
      </>
    );
  }
  if (r.discardedOffsetMs !== null) {
    const proving =
      r.attempt.times.proofStart !== null && r.attempt.times.proofEnd === null;
    return (
      <>
        Dropped at {formatOffset(r.discardedOffsetMs)}
        {proving ? ", during the proof" : null}
      </>
    );
  }
  return <>–</>;
}

function AttemptRow({
  report: r,
  network,
  domain,
  slotMs,
}: {
  report: AttemptReport;
  network: string | null;
  domain: [number, number];
  slotMs: number;
}) {
  const [open, setOpen] = React.useState(false);
  const detailId = React.useId();
  const a = r.attempt;
  const b = a.block;
  const slot = formatCount(a.wonSlot.globalSlot);
  const url = explorerBlockUrl(network, b?.height ?? null);
  const t = a.times;
  const rel = (ms: number | null) =>
    ms !== null && r.slotStartMs !== null ? ms - r.slotStartMs : null;
  const built = rel(t.stagedLedgerDiffStart ?? t.produced);
  const builtEnd = rel(t.produced);
  const proofStart = rel(t.proofStart);
  const proofEnd = rel(t.proofEnd);
  const dropped = r.discardedOffsetMs;
  const ended = r.ended;
  const spans = [
    ...(built !== null && builtEnd !== null
      ? [{ start: built, end: builtEnd, tone: "neutral" as const }]
      : []),
    ...(proofStart !== null && (proofEnd ?? dropped) !== null
      ? [
          {
            start: proofStart,
            end: (proofEnd ?? dropped)!,
            tone: proofEnd !== null ? ("info" as const) : ("warn" as const),
          },
        ]
      : []),
  ];
  const marks = [
    ...(r.broadcastOffsetMs !== null
      ? [{ at: r.broadcastOffsetMs, tone: r.tone }]
      : []),
    ...(r.broadcastOffsetMs === null && dropped !== null
      ? [{ at: dropped, tone: "warn" as const }]
      : []),
  ];
  return (
    <li className="flex flex-col gap-3 px-4 py-4 sm:px-5">
      <div className="grid gap-3 md:grid-cols-[8.5rem_minmax(0,1.15fr)_minmax(0,1fr)] md:gap-5 xl:grid-cols-[8.5rem_minmax(0,1.15fr)_minmax(0,1fr)_7rem]">
        <div className="flex flex-col gap-0.5">
          <span className="font-mono text-sm text-foreground tabular-nums">
            Slot {slot}
          </span>
          <span className="text-xs text-muted-foreground">
            <RelativeTime at={r.slotStartMs} />
          </span>
        </div>
        <div className="flex min-w-0 flex-col items-start gap-1.5">
          <Signal tone={r.tone}>{r.label}</Signal>
          {r.reason ? (
            <p className="text-xs leading-relaxed text-muted-foreground">
              {r.reason}
            </p>
          ) : null}
        </div>
        <div className="flex min-w-0 flex-col gap-1">
          <StageBar
            label={barLabel(r, slotMs)}
            domain={domain}
            window={{ start: 0, end: slotMs }}
            spans={spans}
            marks={marks}
            ticks={[0, slotMs / 3, (2 * slotMs) / 3, slotMs].map(at => ({
              at,
              label: at === 0 ? "0" : seconds(at),
            }))}
          />
          <p className="font-mono text-xs text-foreground tabular-nums">
            <Caption report={r} />
          </p>
        </div>
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 md:col-span-3 xl:col-span-1 xl:flex-col xl:items-end xl:justify-start">
          {b?.height != null && url && ended ? (
            <span className="flex flex-col xl:items-end">
              <ExternalLink
                href={url}
                className="font-mono text-sm"
                title={`Block ${formatHeight(b.height)} on the explorer`}
              >
                {formatHeight(b.height)}
              </ExternalLink>
              {!r.sent || r.outcome === "orphaned" ? (
                <span className="text-xs text-muted-foreground">
                  {r.outcome === "missed"
                    ? "shows the block there"
                    : "shows the winner"}
                </span>
              ) : null}
            </span>
          ) : b?.height != null ? (
            <span className="font-mono text-sm text-foreground">
              {formatHeight(b.height)}
            </span>
          ) : (
            <span className="text-sm text-muted-foreground">No block</span>
          )}
          <Button
            type="button"
            variant="ghost"
            size="sm"
            aria-expanded={open}
            aria-controls={detailId}
            onClick={() => setOpen(o => !o)}
            className="-mr-2"
          >
            Stages
            <span className="sr-only"> of slot {slot}</span>
            <ChevronDown
              aria-hidden
              className={
                open
                  ? "rotate-180 transition-transform"
                  : "transition-transform"
              }
            />
          </Button>
        </div>
      </div>
      <div id={detailId} hidden={!open}>
        {open ? <AttemptDetail report={r} url={url} /> : null}
      </div>
    </li>
  );
}

function AttemptDetail({
  report: r,
  url,
}: {
  report: AttemptReport;
  url: string | null;
}) {
  const a = r.attempt;
  const b = a.block;
  const slot = formatCount(a.wonSlot.globalSlot);
  const steps: TimelineStep[] = r.stages.map(s => ({
    label: s.label,
    time: s.offsetMs !== null ? formatOffset(s.offsetMs) : undefined,
    detail:
      s.key === "won" && s.atMs !== null
        ? `scheduled ${utcDateTime(s.atMs)}`
        : s.detail,
    state: s.state,
  }));
  const ended = r.ended;
  steps.push({
    label: ended ? r.label : "Outcome",
    time:
      r.discardedOffsetMs !== null
        ? formatOffset(r.discardedOffsetMs)
        : undefined,
    detail: r.reason,
    state: ended ? "done" : "pending",
    tone: r.tone,
  });
  const fact = (label: string, value: React.ReactNode) => (
    <div className="grid grid-cols-1 gap-0.5 sm:grid-cols-[9rem_minmax(0,1fr)] sm:gap-4">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 break-words text-foreground">{value}</dd>
    </div>
  );
  return (
    <div className="grid gap-6 border-t pt-4 lg:grid-cols-2">
      <Timeline
        label={`Stages of slot ${slot}, from the slot's start`}
        steps={steps}
      />
      <dl className="flex h-fit flex-col gap-2 font-mono text-[0.8125rem]">
        {fact("Slot start", utcDateTime(r.slotStartMs))}
        {fact("Epoch", formatCount(a.wonSlot.epoch))}
        {a.wonSlot.delegator
          ? fact(
              "Won with the stake of",
              <Hash value={a.wonSlot.delegator} label="delegator" />,
            )
          : null}
        {b ? (
          <>
            {fact(
              "Block",
              b.hash ? (
                <span className="flex flex-col gap-1">
                  <Hash value={b.hash} label="block hash" />
                  {r.sent ? (
                    <TextLink asChild arrow className="w-fit font-sans text-sm">
                      <Link to={`/blocks/${b.hash}`}>In this node</Link>
                    </TextLink>
                  ) : (
                    <span className="font-sans text-xs text-muted-foreground">
                      Never sent: no other node has it.
                    </span>
                  )}
                </span>
              ) : (
                "–"
              ),
            )}
            {fact(
              "Height",
              url ? (
                <ExternalLink href={url} className="w-fit">
                  {formatHeight(b.height)} on the explorer
                </ExternalLink>
              ) : (
                formatHeight(b.height)
              ),
            )}
            {fact(
              "Transactions",
              b.payments + b.delegations + b.zkapps === 0
                ? "None"
                : [
                    plural(b.payments, "payment"),
                    plural(b.delegations, "delegation"),
                    plural(b.zkapps, "zkApp transaction"),
                  ].join(", "),
            )}
            {fact("SNARK work bought", formatCount(b.works))}
            {fact(
              "Coinbase",
              <Amount unit="MINA" value={formatMina(b.coinbase)} />,
            )}
            {fact("Fees", <Amount unit="MINA" value={formatMina(b.fees)} />)}
            {fact(
              "SNARK fees paid",
              <Amount unit="MINA" value={formatMina(b.snarkFees)} />,
            )}
          </>
        ) : null}
      </dl>
    </div>
  );
}
