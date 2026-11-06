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
import { Amount, Measure } from "@minanode/design-system/patterns/amount";
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
import {
  StageBar,
  StageBarAxis,
} from "@minanode/design-system/patterns/stage-bar";
import {
  StatRow,
  type StatRowItem,
} from "@minanode/design-system/patterns/stat-row";
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
import { cn } from "@minanode/design-system/lib/utils";

import { Link } from "../app/router";
import { Count, QUIET_EXTERNAL_LINK } from "../components/cells";
import { Code, Flags } from "../components/code";
import { PHONE_HIDDEN, PhoneList } from "../components/phone-list";
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
        aside={
          <>
            {status.data || stats.data ? (
              <Signal tone={on ? "ok" : "neutral"} className="text-sm">
                {on ? "On" : "Off"}
              </Signal>
            ) : null}
            <PolledAge polled={stats} staleAfterMs={POLL_MS * 4} />
            {/* The one link to the explanation. */}
            <ExternalLink href={docsUrl("producer")} className="text-sm">
              Block producer docs
            </ExternalLink>
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
                  <p>Restart the node with these.</p>
                  <Flags
                    rows={[
                      {
                        label: "Key file",
                        flag: <>--producer-key &lt;file&gt;</>,
                      },
                      {
                        label: "Key password",
                        flag: <>MINA_PRIVKEY_PASS=&lt;password&gt;</>,
                      },
                    ]}
                  />
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

/** A producer's slots, attempts and their fate; minaweb's producer screen
 * shows it too. */
export function Producing({ stats }: { stats: ProducerStats }) {
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
      {ahead.length === 0 && reports.length === 0 ? (
        // No won slot ahead or behind: said once.
        <PageSection title="Won slots">
          <EmptyState
            title="No won slot yet"
            description={<p>None in the slots checked so far.</p>}
          />
        </PageSection>
      ) : (
        <>
          <SlotsAhead ahead={ahead} />
          <Attempts reports={reports} network={network} slotMs={slotMs} />
        </>
      )}
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
    <div className="grid items-start gap-6 lg:grid-cols-2">
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
            prose: !s?.coinbaseReceiver,
          },
          {
            label: "Next won slot",
            // The clock time, then how far off, as the block page's Time.
            value: next ? (
              <span>
                <Measure
                  value={
                    next.slotTimeMs !== null &&
                    next.slotTimeMs - Date.now() < 86_400_000
                      ? utcTime(next.slotTimeMs).slice(0, 5) + " UTC"
                      : utcDateTime(next.slotTimeMs)
                  }
                />{" "}
                <TimeUntil
                  at={next.slotTimeMs}
                  className="font-sans text-muted-foreground"
                />
              </span>
            ) : s?.nextWonSlot ? (
              `Epoch ${s.nextWonSlot.epoch}, slot ${s.nextWonSlot.slot}`
            ) : (
              "None found yet"
            ),
            prose: !next,
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
              stats.epochStart !== null && stats.epochEnd !== null ? (
                <Range
                  from={formatCount(stats.epochStart)}
                  to={formatCount(stats.epochEnd)}
                  word="to"
                />
              ) : (
                "–"
              ),
          },
          {
            label: "Slots checked for wins",
            value: vrf ? (
              <span className="inline-flex flex-col items-end gap-1.5">
                <Range
                  from={formatCount(vrf.evaluated)}
                  to={formatCount(vrf.total)}
                  word="of"
                />
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

/** Two figures and the word between them, the word in the text face. */
function Range({ from, to, word }: { from: string; to: string; word: string }) {
  return (
    <span>
      {from}
      <span className="mx-1.5 font-sans text-muted-foreground">{word}</span>
      {to}
    </span>
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
  const outcomes: StatRowItem[] = [
    {
      label: "Won",
      value: <Count n={sum.won} />,
      detail:
        sum.producing > 0
          ? `${formatCount(sum.ahead)} ahead, ${formatCount(sum.producing)} under way`
          : sum.ahead > 0
            ? `${formatCount(sum.ahead)} ahead`
            : undefined,
    },
    {
      label: "Produced",
      value: <Count n={sent} />,
      detail: "built, proved and sent",
    },
    {
      label: "In the chain",
      value: <Count n={sum.canonical} />,
      detail:
        sent > 0
          ? `${Math.round((sum.canonical / sent) * 100)}% of those sent`
          : "in the best chain",
    },
    {
      label: "Orphaned",
      value: <Count n={sum.orphaned} />,
      detail: "sent, then replaced",
    },
    {
      label: "Lost the tie",
      value: <Count n={sum.lostTie} />,
      detail: "another block won",
    },
    {
      label: "Missed",
      value: <Count n={sum.missed} />,
      detail: sum.missed > 0 ? "the slot passed" : undefined,
    },
  ];
  const times: StatRowItem[] = [
    {
      label: "Median proof",
      value: (
        <Measure
          value={sum.proof ? formatDuration(sum.proof.median) : null}
          unitClassName="font-sans text-sm font-normal"
        />
      ),
      detail: sum.proof
        ? `over ${plural(sum.proof.count, "proof")}`
        : "no proof finished",
    },
    {
      label: "Worst proof",
      value: (
        <Measure
          value={sum.proof ? formatDuration(sum.proof.worst) : null}
          unitClassName="font-sans text-sm font-normal"
        />
      ),
      detail: `in a ${seconds(slotMs)} slot`,
    },
    {
      label: "Median broadcast",
      value: (
        <Measure
          value={sum.broadcast ? formatOffset(sum.broadcast.median) : null}
          unitClassName="font-sans text-sm font-normal"
        />
      ),
      detail: sum.broadcast
        ? `from the slot's start; latest ${formatOffset(sum.broadcast.worst)}`
        : "from the slot's start",
    },
    {
      label: "Coinbase earned",
      value: (
        <Amount
          unit="MINA"
          value={formatMina(sum.coinbase)}
          unitClassName="font-sans text-sm font-normal"
        />
      ),
      detail:
        sum.canonical > 0
          ? `fees +${formatMina(sum.fees)}, SNARK work −${formatMina(sum.snarkFees)}`
          : "from blocks in the chain",
    },
  ];
  return (
    <PageSection
      title="Summary"
      description={
        scope === "all" && kept
          ? `From ${utcDateTime(sum.sinceMs)}${beforeRestart ? ", before the latest restart" : ""}.`
          : undefined
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
      {/* Two groups, a card's gap apart: their dividers needn't line up.
          On a phone each is a panel, its captions notes under the labels,
          so no tile holds a caption wrapped to three lines. */}
      <div className="hidden flex-col gap-6 sm:flex">
        <StatRow label="Outcomes" items={outcomes} />
        <StatRow label="Times and coinbase" items={times} />
      </div>
      <div className="flex flex-col gap-4 sm:hidden">
        <KeyValueList
          title="Outcomes"
          rows={outcomes.map(asRow)}
          className={ROW_UNITS}
        />
        <KeyValueList
          title="Times and coinbase"
          rows={times.map(asRow)}
          className={ROW_UNITS}
        />
      </div>
    </PageSection>
  );
}

/** In a row, a big figure's Inter unit goes back to mono at the row's
 * size, as every row's unit is. */
const ROW_UNITS =
  "[&_[data-slot=amount]_.font-sans]:font-mono [&_[data-slot=amount]_.font-sans]:text-[0.8125rem]";

/** A stat as a panel row: its caption a note under the label. */
const asRow = (item: StatRowItem) => ({
  label: item.label,
  value: item.value,
  note: item.detail,
});

/** A figure, muted at zero, as the table zeros are. */
function Fig({ zero, children }: { zero: boolean; children: React.ReactNode }) {
  return (
    <span className={zero ? "text-muted-foreground" : undefined}>
      {children}
    </span>
  );
}

/** Nothing expected (no stake): no range to fall within. */
const nothingToJudge = (e: EpochExpectation) => e.expected === 0;

/** Where the count fell against the 95% range, and, with few wins
 * expected, that it says little either way. */
function verdict(e: EpochExpectation): string {
  const place =
    e.found < e.low
      ? "below the range"
      : e.found > e.high
        ? "above the range"
        : "within the range";
  return e.expected < 5 ? `${place}; too few to judge` : place;
}

function Expectations({ stats }: { stats: ProducerStats }) {
  const rows = epochExpectations(stats.vrfEpochs);
  return (
    <PageSection title="Expected against found">
      {!reportsStake(stats.vrfEpochs) || rows.length === 0 ? (
        <EmptyState
          title="Not reported by this node"
          description={
            <p>
              This build sends no stake with its slot search (
              <Code>vrf_stats</Code> has no <Code>delegated_stake</Code>). Newer
              builds do.
            </p>
          }
        />
      ) : (
        <>
          {/* On a phone an epoch a panel, Expected and Found together. */}
          <div className="flex flex-col gap-4 sm:hidden">
            {rows
              .slice()
              .reverse()
              .map(e => (
                <KeyValueList
                  key={e.epoch}
                  title={`Epoch ${formatCount(e.epoch)}`}
                  rows={[
                    {
                      label: "Expected",
                      note:
                        e.expected > 0
                          ? `95% range: ${formatCount(e.low)} to ${formatCount(e.high)}`
                          : undefined,
                      value: (
                        <Fig zero={e.expected === 0}>
                          {e.expected.toFixed(1)}
                        </Fig>
                      ),
                    },
                    {
                      label: "Found",
                      note: nothingToJudge(e) ? undefined : verdict(e),
                      value: (
                        <Fig zero={e.found === 0}>{formatCount(e.found)}</Fig>
                      ),
                      tone: "brand",
                    },
                    {
                      label: "Stake",
                      value: (
                        <Amount
                          unit="MINA"
                          value={formatCount(
                            Math.round(e.delegatedStake / 1_000_000_000),
                          )}
                        />
                      ),
                    },
                    {
                      label: "Slots checked",
                      value: formatCount(e.slotsChecked),
                    },
                  ]}
                />
              ))}
          </div>
          {/* Fixed columns: the figures sit together, Epoch takes the
              slack. */}
          <Table
            className="table-fixed [&_td]:align-top"
            containerProps={{
              ...tableRegion("Expected against found"),
              className: "hidden sm:block",
            }}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Epoch</TableHead>
                <TableHead className="w-[19rem] text-right">
                  Stake (MINA)
                </TableHead>
                <TableHead className="w-[8.5rem] text-right">
                  Slots checked
                </TableHead>
                <TableHead className="w-[11rem] text-right">Expected</TableHead>
                <TableHead className="w-[10rem] text-right">Found</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows
                .slice()
                .reverse()
                .map(e => (
                  <TableRow key={e.epoch}>
                    <TableCell className="font-mono tabular-nums">
                      {formatCount(e.epoch)}
                    </TableCell>
                    <TableCell className="text-right">
                      {/* Whole MINA: a stake's nanomina pass 2^53, past
                        exact JSON numbers, but whole MINA stay exact. */}
                      <span
                        className={cn(
                          "font-mono tabular-nums",
                          e.delegatedStake === 0 && "text-muted-foreground",
                        )}
                      >
                        {formatCount(
                          Math.round(e.delegatedStake / 1_000_000_000),
                        )}
                      </span>
                      {/* A zero stake needs no share. */}
                      {e.delegatedStake > 0 ? (
                        <span className="block pt-1 text-xs whitespace-nowrap text-muted-foreground">
                          {(e.share * 100).toFixed(2)}% of stake
                          {e.delegators !== null
                            ? `, ${plural(e.delegators, "delegator")}`
                            : ""}
                        </span>
                      ) : null}
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
                      <Fig zero={e.expected === 0}>{e.expected.toFixed(1)}</Fig>
                      {e.expected > 0 ? (
                        <span className="block pt-1 font-sans text-xs text-muted-foreground">
                          95% range: {formatCount(e.low)} to{" "}
                          {formatCount(e.high)}
                        </span>
                      ) : null}
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      <Fig zero={e.found === 0}>{formatCount(e.found)}</Fig>
                      {nothingToJudge(e) ? null : (
                        <span className="block pt-1 font-sans text-xs text-muted-foreground">
                          {verdict(e)}
                        </span>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
            </TableBody>
          </Table>
        </>
      )}
    </PageSection>
  );
}

function SlotsAhead({ ahead }: { ahead: UpcomingSlot[] }) {
  const { shown, hidden, toggle } = useShowAll(ahead, "soonest");
  return (
    <PageSection title="Won slots ahead" aside={toggle}>
      {ahead.length === 0 ? (
        <EmptyState
          title="No won slot ahead"
          description={<p>None in the slots checked so far.</p>}
        />
      ) : (
        <>
          {/* On a phone a slot a row: its time and whether it is scheduled,
            then how soon, its global slot and its epoch. */}
          <PhoneList
            label="Won slots ahead"
            rows={shown.map((w, i) => ({
              key: w.globalSlot ?? w.slotTimeMs ?? i,
              lead: (
                <span className="font-mono tabular-nums">
                  <Measure value={utcDateTime(w.slotTimeMs)} />
                </span>
              ),
              end: w.scheduled ? (
                <Signal variant="plain" tone="info">
                  Scheduled
                </Signal>
              ) : null,
              meta: [
                <TimeUntil key="t" at={w.slotTimeMs} />,
                <>
                  slot{" "}
                  <span className="font-mono text-foreground">
                    {formatCount(w.globalSlot)}
                  </span>
                </>,
                <>
                  epoch{" "}
                  <span className="font-mono text-foreground">
                    {formatCount(w.epoch)}
                  </span>
                </>,
              ],
            }))}
          />
          <Table
            className="[&_td]:align-middle [&_td]:whitespace-nowrap"
            containerProps={{
              ...tableRegion("Won slots ahead"),
              className: PHONE_HIDDEN,
            }}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Slot time</TableHead>
                <TableHead>Starts in</TableHead>
                <TableHead className="text-right">Global slot</TableHead>
                <TableHead className="text-right">Epoch</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {shown.map(w => (
                <TableRow key={w.globalSlot ?? w.slotTimeMs}>
                  <TableCell className="font-mono tabular-nums">
                    <Measure value={utcDateTime(w.slotTimeMs)} />
                  </TableCell>
                  <TableCell>
                    <span className="inline-flex items-center gap-3">
                      <TimeUntil at={w.slotTimeMs} />
                      {/* The next slot's attempt is ready: said on its row. */}
                      {w.scheduled ? (
                        <Signal variant="plain" tone="info">
                          Scheduled
                        </Signal>
                      ) : null}
                    </span>
                  </TableCell>
                  <TableCell className="text-right font-mono tabular-nums">
                    {formatCount(w.globalSlot)}
                  </TableCell>
                  <TableCell className="text-right font-mono tabular-nums">
                    {formatCount(w.epoch)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </>
      )}
      {hidden > 0 ? (
        <p className="text-sm text-balance text-muted-foreground">
          The first {shown.length} of {formatCount(ahead.length)}; the last on{" "}
          {/* The time kept whole: no "UTC." alone on a line. */}
          <span className="whitespace-nowrap">
            {utcDateTime(ahead[ahead.length - 1].slotTimeMs)}.
          </span>
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
        // A ring, so the faintest swatch shows on white.
        className={`inline-block ring-1 ring-border ring-inset ${round ? "size-2.5 rounded-full" : "h-1.5 w-4 rounded-full"} ${fill}`}
      />
      {text}
    </span>
  );
  // The slot as the bar draws it: its fill over the track's, on the card's
  // colour, so the swatch and the bar match on any page background.
  const slot = (
    <span className="inline-flex items-center gap-1.5">
      <span
        aria-hidden
        className="relative inline-block h-1.5 w-4 overflow-hidden rounded-full bg-card"
      >
        <span className="block size-full bg-muted-foreground/12">
          <span className="block size-full bg-muted-foreground/25" />
        </span>
        {/* The other swatches' inset ring, over the fills. */}
        <span className="absolute inset-0 rounded-full ring-1 ring-border ring-inset" />
      </span>
      the slot
    </span>
  );
  return (
    <p className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
      {slot}
      {item(toneFill.neutral, "building")}
      {item(toneFill.info, "proving")}
      {item(toneFill.ok, "sent, in the chain", true)}
      {item(toneFill.warn, "dropped or replaced", true)}
    </p>
  );
}

/** The won-slot list's columns: slot, outcome, the bar, the block. */
// On a phone the slot and its outcome share the first line; the bar and
// the height take the width under them.
const ATTEMPT_GRID =
  "grid grid-cols-[minmax(0,1fr)_auto] gap-x-4 gap-y-3 md:grid-cols-[8.5rem_minmax(0,1fr)_minmax(0,1.15fr)] md:gap-5 xl:grid-cols-[8.5rem_minmax(0,1fr)_minmax(0,1.15fr)_10rem]";

/** The bar's ticks: the slot's start, its thirds and its end. */
const slotTicks = (slotMs: number) =>
  [0, slotMs / 3, (2 * slotMs) / 3, slotMs].map(at => ({
    at,
    label: at === 0 ? "0" : seconds(at),
  }));

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
    <PageSection title="Won slots" aside={toggle}>
      {reports.length === 0 ? (
        <EmptyState
          title="No won slot has come yet"
          description={<p>Each appears here once its slot comes up.</p>}
        />
      ) : (
        <>
          <Legend />
          <div className="overflow-hidden rounded-xl border bg-card">
            {/* The bar's one axis, over every row: on a phone alone, from
                768 px with the columns' heads, as a table's head row. */}
            <div
              aria-hidden
              className="border-b bg-muted/40 px-4 py-2.5 md:hidden"
            >
              <StageBarAxis domain={domain} ticks={slotTicks(slotMs)} />
            </div>
            <div
              aria-hidden
              className={`${ATTEMPT_GRID} hidden items-center border-b bg-muted/40 px-4 py-2.5 text-xs font-medium text-muted-foreground md:grid`}
            >
              <span>Slot</span>
              <span>Outcome</span>
              <StageBarAxis domain={domain} ticks={slotTicks(slotMs)} />
              {/* Over the heights: an invisible twin of the Stages button
                  holds its width. */}
              <span className="hidden items-center justify-end gap-x-2 xl:flex">
                Block
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  tabIndex={-1}
                  aria-hidden
                  className="invisible -mr-2"
                >
                  Stages
                  <ChevronDown aria-hidden />
                </Button>
              </span>
            </div>
            <ul className="flex flex-col divide-y">
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
          </div>
        </>
      )}
    </PageSection>
  );
}

/** "11-05 23:40:12 UTC": a day's date and the clock to the second, so a
 * stage on another day keeps the precision of the others. */
/** A stage on another day than its slot: the full date, as Slot start
 * writes it, with seconds. */
function utcDateSeconds(ms: number): string {
  const iso = new Date(ms).toISOString();
  return `${iso.slice(0, 10)} ${iso.slice(11, 19)} UTC`;
}

/** A stage's time, its unit ("s", "UTC") muted. */
function stageTime(text: string) {
  return <Measure value={text} />;
}

function sameUtcDay(a: number, b: number | null): boolean {
  return (
    b !== null &&
    new Date(a).toISOString().slice(0, 10) ===
      new Date(b).toISOString().slice(0, 10)
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
  // The slot's cell says when it starts.
  if (r.outcome === "upcoming") return null;
  if (r.outcome === "producing") return <>Under way</>;
  if (r.outcome === "previousRun") return <>From before the restart</>;
  // The row's note says the restart; the empty bar shows it.
  if (r.interrupted) return null;
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
    // A block in the chain marks teal, as the legend says, pending or not.
    ...(r.broadcastOffsetMs !== null
      ? [
          {
            at: r.broadcastOffsetMs,
            tone: r.outcome === "canonical" ? ("ok" as const) : r.tone,
          },
        ]
      : []),
    ...(r.broadcastOffsetMs === null && dropped !== null
      ? [{ at: dropped, tone: "warn" as const }]
      : []),
  ];
  return (
    <li className="flex flex-col gap-3 px-4 py-4">
      <div className={ATTEMPT_GRID}>
        <div className="flex flex-col gap-0.5">
          <span className="text-sm text-foreground">
            Slot <span className="font-mono tabular-nums">{slot}</span>
          </span>
          <span className="text-xs text-muted-foreground">
            <RelativeTime at={r.slotStartMs} />
          </span>
        </div>
        <div className="flex min-w-0 flex-col items-end gap-0.5 text-right md:items-start md:text-left">
          <Signal variant="plain" tone={r.tone}>
            {r.label}
          </Signal>
          {r.note ? (
            <p className="text-xs text-muted-foreground">{r.note}</p>
          ) : null}
        </div>
        <div className="col-span-2 flex min-w-0 flex-col gap-1 md:col-span-1">
          <StageBar
            label={barLabel(r, slotMs)}
            domain={domain}
            window={{ start: 0, end: slotMs }}
            spans={spans}
            marks={marks}
          />
          <p className="text-xs text-foreground tabular-nums empty:hidden">
            <Caption report={r} />
          </p>
        </div>
        {/* The height and Stages on one line, so every row is two lines. */}
        <div className="col-span-2 flex flex-wrap items-center justify-between gap-x-4 gap-y-1 md:col-span-3 xl:col-span-1 xl:justify-end xl:gap-x-2 xl:self-start">
          {b?.height != null && url && ended ? (
            <ExternalLink
              href={url}
              className={cn(QUIET_EXTERNAL_LINK, "font-mono text-sm")}
              title={`Block ${formatHeight(b.height)} on the explorer${
                r.outcome === "missed"
                  ? ": the block made at that height"
                  : !r.sent || r.outcome === "orphaned"
                    ? ": the block that won"
                    : ""
              }`}
            >
              {formatHeight(b.height)}
            </ExternalLink>
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
    // Scheduled can be hours before the slot: its clock time, with the
    // date when that differs from the slot's.
    time:
      s.key === "scheduled"
        ? s.atMs !== null
          ? // "UTC" muted, as a unit is.
            stageTime(
              sameUtcDay(s.atMs, r.slotStartMs)
                ? utcTime(s.atMs)
                : utcDateSeconds(s.atMs),
            )
          : undefined
        : s.offsetMs !== null
          ? // Figures in mono, the unit muted, as every duration.
            stageTime(formatOffset(s.offsetMs))
          : undefined,
    detail: s.detail,
    state: s.state,
  }));
  const ended = r.ended;
  steps.push({
    label: ended ? r.label : "Outcome",
    time:
      r.discardedOffsetMs !== null
        ? stageTime(formatOffset(r.discardedOffsetMs))
        : undefined,
    // A lost tie's row already says it; the others add the height or why.
    detail: r.outcome === "lostTie" || r.outcome === "missed" ? null : r.reason,
    state: ended ? "done" : "pending",
    tone: r.tone,
  });
  // Figures in mono; `prose` for a value that is words. On a phone the
  // panels' row, its value at the right; from 640 px a label column.
  const fact = (label: string, value: React.ReactNode, prose = false) => (
    <div className="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-4 sm:grid-cols-[11rem_minmax(0,1fr)]">
      <dt className="text-muted-foreground">{label}</dt>
      <dd
        className={cn(
          "min-w-0 break-words text-right text-foreground sm:text-left",
          !prose && "font-mono text-[0.8125rem] leading-5 tabular-nums",
        )}
      >
        {value}
      </dd>
    </div>
  );
  return (
    // On the row's own columns from 1280 px: the stages under the slot and
    // its outcome, the facts under the bar, behind a hairline.
    <div className="grid gap-6 border-t pt-4 lg:grid-cols-2 lg:gap-x-12 xl:grid-cols-[8.5rem_minmax(0,1fr)_minmax(0,1.15fr)_10rem] xl:gap-x-5">
      <div className="xl:col-span-2">
        <Timeline
          label={`Stages of slot ${slot}, from the slot's start`}
          steps={steps}
        />
      </div>
      {/* Its own height, so a short list (a missed slot) leaves no ruled
          empty column. */}
      <dl className="flex flex-col gap-3 border-t pt-4 text-sm lg:self-start lg:border-t-0 lg:border-l lg:pt-0 lg:pl-6 xl:col-span-2">
        {fact("Slot start", <Measure value={utcDateTime(r.slotStartMs)} />)}
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
                <span className="flex flex-col items-end gap-1 sm:items-start">
                  <Hash value={b.hash} label="block hash" />
                  {r.sent ? (
                    <TextLink asChild arrow className="w-fit font-sans text-xs">
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
              true,
            )}
            {fact(
              "Height",
              url ? (
                <ExternalLink
                  href={url}
                  className={cn(QUIET_EXTERNAL_LINK, "w-fit font-mono")}
                >
                  {formatHeight(b.height)}
                </ExternalLink>
              ) : (
                formatHeight(b.height)
              ),
            )}
            {fact(
              "Transactions",
              b.payments + b.delegations + b.zkapps === 0 ? (
                // Muted, as a zero is.
                <span className="text-muted-foreground">None</span>
              ) : (
                // The kinds it has: no "0 delegations".
                [
                  b.payments ? plural(b.payments, "payment") : null,
                  b.delegations ? plural(b.delegations, "delegation") : null,
                  b.zkapps ? plural(b.zkapps, "zkApp transaction") : null,
                ]
                  .filter(Boolean)
                  .join(", ")
              ),
              true,
            )}
            {fact("SNARK work bought", <Count n={b.works} />)}
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
