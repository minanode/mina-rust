import * as React from "react";
import {
  CircleCheck,
  Cpu,
  Lock,
  Play,
  Power,
  TriangleAlert,
} from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
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
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { Signal } from "@minanode/design-system/patterns/signal";

import { Amount, Measure } from "@minanode/design-system/patterns/amount";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { StatRow } from "@minanode/design-system/patterns/stat-row";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";

import {
  describeError,
  describeJob,
  explorerBlockUrl,
  feeInput,
  formatCount,
  formatHeight,
  formatMina,
  confirmation,
  confirmationLabel,
  confirmations,
  confirmationTone,
  confirmationTotals,
  CONFIRMATIONS,
  feeComparison,
  marketProvers,
  NodeError,
  plural,
  shortHash,
  startChange,
  STOP_CHANGE,
  strategyLabel,
  workerView,
  type IncludedWork,
  type SnarkMarket,
  type WorkerChange,
  type WorkerControl,
} from "@minanode/node-client";
import {
  useIncludedWork,
  useNodeClient,
  useNodeStatus,
  useSyncView,
  useWorkerControl,
  type Polled,
} from "@minanode/node-client/react";
import { useNow } from "@minanode/design-system/lib/time";
import { cn } from "@minanode/design-system/lib/utils";

import { QUIET_EXTERNAL_LINK } from "../components/cells";
import { Code, Flags } from "../components/code";
import { CountOf, PHONE_HIDDEN, PhoneList } from "../components/phone-list";
import { useShowAll } from "../components/show-all";
import { Loaded, PolledAge } from "../components/states";
import { docsUrl } from "../lib/docs";

/** Both answers poll together, so the page has one age. */
const POLL_MS = 5_000;

/** The age of two polled answers as one: the older, and either failure. */
function together(a: Polled<unknown>, b: Polled<unknown>) {
  return {
    updatedAt:
      a.updatedAt !== null && b.updatedAt !== null
        ? Math.min(a.updatedAt, b.updatedAt)
        : (a.updatedAt ?? b.updatedAt),
    loading: a.loading || b.loading,
    error: a.error ?? b.error,
    refresh: () => {
      a.refresh();
      b.refresh();
    },
  };
}

export function WorkerPage() {
  const control = useWorkerControl(POLL_MS);
  const included = useIncludedWork(POLL_MS);
  const { status } = useNodeStatus();
  // The market is the node's view of the chain: only once it follows it.
  const synced = useSyncView(useNow()).key === "Synced";
  const c = control.data;
  const view = c ? workerView(c) : null;
  const work = included.data;
  const off = view?.label === "Off";
  // A worker that never ran has no log to show.
  const ran = work ? work.keys.length > 0 || work.entries.length > 0 : false;
  // The log's own tip; an older node doesn't send one.
  const tip =
    work?.bestTipHeight ??
    c?.bestTipHeight ??
    status.data?.bestTip?.height ??
    null;

  return (
    <>
      <PageHeader
        eyebrow="Roles"
        title="SNARK worker"
        aside={
          <>
            {view ? (
              <Signal
                tone={view.tone}
                pulse={view.label === "Working"}
                className="text-sm"
              >
                {view.label}
              </Signal>
            ) : null}
            <PolledAge
              polled={together(control, included)}
              staleAfterMs={POLL_MS * 4}
            />
            {/* The one link to the explanation. */}
            <ExternalLink href={docsUrl("worker")} className="text-sm">
              SNARK worker docs
            </ExternalLink>
          </>
        }
      />

      <Loaded polled={control} what="the worker's state" rows={5}>
        {c =>
          workerView(c).label === "Off" ? (
            <WorkerOff
              control={c}
              market={work?.market}
              onChanged={control.refresh}
            />
          ) : (
            <div className="grid items-start gap-6 lg:grid-cols-2">
              <WorkerStatus control={c} jobs={status.data?.snarkPool.jobs} />
              {c.allowed ? (
                <ControlCard control={c} onChanged={control.refresh} />
              ) : (
                <ControlOff />
              )}
            </div>
          )
        }
      </Loaded>

      {c && c.recent.length > 0 ? <RecentJobs control={c} /> : null}

      {c && (!off || ran) ? (
        <IncludedSection included={included} tip={tip} />
      ) : null}

      {work?.market && synced ? (
        <Market
          market={work.market}
          network={work.network}
          ownKeys={work.keys}
          ourKey={c && !off ? c.publicKey : null}
          ourFee={c && !off ? c.fee : null}
        />
      ) : null}
    </>
  );
}

/** The worker is off: one panel that says so and how to turn it on, with
 * the start form when this page may start it. */
function WorkerOff({
  control: c,
  market,
  onChanged,
}: {
  control: WorkerControl;
  market?: SnarkMarket | null;
  onChanged: () => void;
}) {
  if (c.allowed) {
    return <ControlCard control={c} market={market} onChanged={onChanged} />;
  }
  return (
    <EmptyState
      icon={<Cpu />}
      title="The SNARK worker is off"
      description={
        <>
          <p>Restart the node with these.</p>
          <Flags
            rows={[
              {
                label: "Worker key",
                flag: <>--run-snarker &lt;public key&gt;</>,
              },
              { label: "Fee", flag: <>--snarker-fee &lt;MINA&gt;</> },
              {
                label: "Dashboard control",
                flag: "--allow-worker-control",
              },
            ]}
          />
          <p className="mt-3">{CONTROL_WARNING}</p>
        </>
      }
    />
  );
}

/** Control changes the fee receiver: say so wherever it is offered. */
const CONTROL_WARNING =
  "Control changes where fees go: keep the node's HTTP port reachable only by you.";

/** The worker runs, but this page may not change it. */
function ControlOff() {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Lock aria-hidden className="size-4 text-brand-text" />
          Control is off on this node
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 text-sm leading-relaxed text-muted-foreground">
        <p>
          To stop or re-price it from this page, restart the node with{" "}
          <Code>--allow-worker-control</Code>.
        </p>
        <p>{CONTROL_WARNING}</p>
      </CardContent>
    </Card>
  );
}

export function WorkerStatus({
  control: c,
  jobs,
  aside,
}: {
  control: WorkerControl;
  /** Jobs at the tip, from the node's status. */
  jobs?: number | null;
  /** In the card's head: the worker's state, where a page shows it there. */
  aside?: React.ReactNode;
}) {
  const view = workerView(c);
  const rows: {
    label: string;
    value: React.ReactNode;
    tone?: "brand";
    prose?: boolean;
    note?: React.ReactNode;
  }[] = [
    {
      label: "State",
      prose: true,
      // When the proof started is the row's note: the value stays one line.
      note:
        view.label === "Working" && c.job?.submittedMs ? (
          <>
            started <TimeAgo at={c.job.submittedMs} />
          </>
        ) : undefined,
      value:
        view.label === "Working" && c.job ? (
          <span>
            Proving: {describeJob(c.job.summary, c.job.zkapp).toLowerCase()}
          </span>
        ) : (
          // A value, not a sentence: no full stop.
          view.detail.replace(/\.$/, "")
        ),
    },
  ];
  if (c.publicKey) {
    rows.push({
      label: "Fees go to",
      value: (
        <Hash
          value={c.publicKey}
          label="fee receiver key"
          className="sm:justify-end"
        />
      ),
    });
    rows.push({
      label: "Its account",
      prose: true,
      value:
        c.accountExists === null
          ? "Not known yet"
          : c.accountExists
            ? "In the ledger"
            : "No account yet",
    });
  }
  if (c.fee !== null) {
    rows.push({
      label: "Fee per job",
      value: <Amount unit="MINA" value={formatMina(c.fee)} />,
      tone: "brand",
    });
  }
  if (c.strategy) {
    rows.push({
      label: "Picks jobs",
      value: strategyLabel(c.strategy),
      prose: true,
    });
  }
  if (c.proofsMade !== null) {
    rows.push({
      label: "Proofs since the node started",
      value: formatCount(c.proofsMade),
    });
  }
  if (jobs !== null && jobs !== undefined) {
    rows.push({
      label: "Jobs the chain needs",
      value: formatCount(jobs),
      // The zkApp share is a part of them, not a count of its own.
      note:
        c.zkappJobs !== null
          ? `${formatCount(c.zkappJobs)} for zkApp transactions`
          : undefined,
    });
  } else if (c.zkappJobs !== null) {
    rows.push({
      label: "Jobs for zkApp transactions",
      value: formatCount(c.zkappJobs),
    });
  }
  return (
    <div className="flex flex-col gap-4">
      <KeyValueList title="Worker" rows={rows} aside={aside} />
      {c.feeProblem ? (
        <Alert variant="caution">
          <TriangleAlert aria-hidden />
          <AlertTitle>Blocks won't buy this work</AlertTitle>
          <AlertDescription>
            <p>{c.feeProblem}</p>
          </AlertDescription>
        </Alert>
      ) : null}
      {c.settingWarning ? (
        <Alert variant="caution">
          <TriangleAlert aria-hidden />
          <AlertTitle>The saved setting wasn't used at startup</AlertTitle>
          <AlertDescription>
            <p>{c.settingWarning}</p>
          </AlertDescription>
        </Alert>
      ) : null}
    </div>
  );
}

type Outcome =
  | { kind: "idle" }
  | { kind: "sending"; action: "start" | "update" | "stop" }
  | { kind: "done"; message: string }
  | { kind: "failed"; message: string; origin: boolean };

function ControlCard({
  control: c,
  market,
  onChanged,
}: {
  control: WorkerControl;
  market?: SnarkMarket | null;
  onChanged: () => void;
}) {
  const [publicKey, setPublicKey] = React.useState(c.publicKey ?? "");
  // The market may arrive after the control answer. Follow it until the
  // operator edits the fee, then keep their input across polling updates.
  const [enteredFee, setFee] = React.useState<string | null>(null);
  const lowestOffer =
    market?.offers.reduce<number | null>(
      (lowest, offer) =>
        Number.isSafeInteger(offer.minFee) &&
        offer.minFee >= 0 &&
        offer.bundles > 0
          ? Math.min(lowest ?? offer.minFee, offer.minFee)
          : lowest,
      null,
    ) ?? null;
  const minimumFee =
    c.accountExists === false && publicKey.trim() === c.publicKey
      ? (c.accountCreationFee ?? 1_000_000_000)
      : 0;
  const suggestedFee = formatMina(
    Math.max(minimumFee, lowestOffer ?? 100_000_000),
  )!;
  const fee =
    enteredFee ?? (c.enabled ? feeInput(c) : suggestedFee.replace(/,/g, ""));
  const [fieldError, setFieldError] = React.useState<{
    field: string;
    message: string;
  } | null>(null);
  const [outcome, setOutcome] = React.useState<Outcome>({ kind: "idle" });
  const [confirmStop, setConfirmStop] = React.useState(false);
  const client = useNodeClient();
  const keyId = React.useId();
  const feeId = React.useId();
  const feeHintId = React.useId();
  const errorId = React.useId();

  const running = c.enabled;
  const unchanged =
    running &&
    publicKey.trim() === (c.publicKey ?? "") &&
    fee.trim() === feeInput(c);

  async function send(
    change: WorkerChange,
    action: "start" | "update" | "stop",
  ) {
    setOutcome({ kind: "sending", action });
    try {
      const after = await client.changeWorker(change);
      setOutcome({
        kind: "done",
        message:
          action === "stop"
            ? "The worker is stopping. Its saved setting is removed."
            : `${action === "start" ? "Worker started" : "Worker updated"}: ${formatMina(after.fee) ?? fee} MINA a job, fees to ${shortHash(after.publicKey)}.`,
      });
      if (action === "stop") setConfirmStop(false);
      onChanged();
    } catch (error) {
      const message = describeError(error);
      setOutcome({
        kind: "failed",
        message,
        origin:
          error instanceof NodeError &&
          error.status === 403 &&
          /origin/i.test(message),
      });
    }
  }

  function submit(event: React.FormEvent) {
    event.preventDefault();
    const check = startChange(publicKey, fee);
    if (!check.ok) {
      setFieldError({ field: check.field, message: check.message });
      document
        .getElementById(check.field === "public_key" ? keyId : feeId)
        ?.focus();
      return;
    }
    setFieldError(null);
    void send(check.change, running ? "update" : "start");
  }

  const sending = outcome.kind === "sending";

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {running ? "Change the worker" : "Start the worker"}
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <form
          noValidate
          onSubmit={submit}
          className="flex max-w-3xl flex-col gap-4"
          aria-describedby={fieldError ? errorId : undefined}
        >
          <div className="flex flex-col gap-2">
            <Label htmlFor={keyId}>Fee receiver public key</Label>
            <Input
              id={keyId}
              name="public_key"
              value={publicKey}
              onChange={e => setPublicKey(e.target.value)}
              placeholder="B62q…"
              autoComplete="off"
              spellCheck={false}
              className="font-mono"
              aria-invalid={fieldError?.field === "public_key" || undefined}
              aria-describedby={
                fieldError?.field === "public_key" ? errorId : undefined
              }
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor={feeId}>Fee per job, in MINA</Label>
            <Input
              id={feeId}
              name="fee"
              value={fee}
              onChange={e => setFee(e.target.value)}
              inputMode="decimal"
              autoComplete="off"
              className="max-w-48 font-mono"
              aria-invalid={fieldError?.field === "fee" || undefined}
              aria-describedby={
                fieldError?.field === "fee"
                  ? `${feeHintId} ${errorId}`
                  : feeHintId
              }
            />
            {/* What bounds the fee, under it. */}
            <p
              id={feeHintId}
              className="text-sm text-pretty text-muted-foreground empty:hidden"
            >
              {!running && lowestOffer !== null
                ? `Lowest offer in the pool now: ${formatMina(lowestOffer)} MINA. `
                : null}
              {c.accountCreationFee !== null
                ? `A key that has no account yet needs a fee of at least ${formatMina(c.accountCreationFee)} MINA.`
                : null}
            </p>
          </div>
          {fieldError ? (
            <p id={errorId} className="text-sm text-destructive">
              {fieldError.message}
            </p>
          ) : null}
          <div className="flex flex-wrap items-center gap-2 pt-1">
            <Button
              type="submit"
              disabled={sending || unchanged}
              aria-busy={sending}
            >
              {running ? <CircleCheck aria-hidden /> : <Play aria-hidden />}
              {sending && outcome.action !== "stop"
                ? running
                  ? "Applying…"
                  : "Starting…"
                : running
                  ? "Apply changes"
                  : "Start worker"}
            </Button>
            {running && !confirmStop ? (
              <Button
                type="button"
                variant="outline"
                disabled={sending}
                onClick={() => setConfirmStop(true)}
              >
                <Power aria-hidden />
                Stop worker
              </Button>
            ) : null}
          </div>
          {running && confirmStop ? (
            <InlineConfirm
              label="Confirm stopping the worker"
              message="Stop the worker? A proof in progress is dropped, and the node forgets the setting."
              confirmLabel="Stop worker"
              busyLabel={
                outcome.kind === "sending" && outcome.action === "stop"
                  ? "Stopping…"
                  : undefined
              }
              cancelLabel="Keep it running"
              busy={sending}
              onConfirm={() => void send(STOP_CHANGE, "stop")}
              onCancel={() => setConfirmStop(false)}
            />
          ) : null}
        </form>
        <div aria-live="polite" className="empty:hidden">
          {outcome.kind === "done" ? (
            <Alert variant="brand">
              <CircleCheck aria-hidden />
              <AlertTitle>Done</AlertTitle>
              <AlertDescription>
                <p>{outcome.message}</p>
              </AlertDescription>
            </Alert>
          ) : null}
          {outcome.kind === "failed" ? (
            <Alert variant="destructive">
              <TriangleAlert aria-hidden />
              <AlertTitle>The node refused the change</AlertTitle>
              <AlertDescription>
                <p>{outcome.message}</p>
                {outcome.origin ? (
                  <p>
                    Open the dashboard at the node's own address (its IP or
                    localhost), or start the node with{" "}
                    <Code>--worker-control-origin</Code> for this page's origin.
                  </p>
                ) : null}
              </AlertDescription>
            </Alert>
          ) : null}
        </div>
      </CardContent>
    </Card>
  );
}

export function RecentJobs({ control: c }: { control: WorkerControl }) {
  const { shown, toggle } = useShowAll(c.recent, "newest");
  return (
    <PageSection title="Recent jobs" aside={toggle}>
      {/* On a phone a job a row: the job and its result, then when, how
          long and the fee. */}
      <ul
        aria-label="Recent jobs"
        className="flex flex-col divide-y rounded-xl border bg-card sm:hidden"
      >
        {shown.map((o, i) => (
          <li
            key={`${o.jobId}-${o.timeMs}-${i}`}
            className="flex flex-col gap-1 px-4 py-3"
          >
            <div className="flex items-center justify-between gap-3 text-sm">
              <span>{describeJob(o.summary, o.zkapp)}</span>
              <JobResult job={o} />
            </div>
            <p className="flex flex-wrap gap-x-3 text-xs text-muted-foreground tabular-nums">
              <TimeAgo at={o.timeMs} />
              {o.durationMs !== null ? (
                <span>
                  took{" "}
                  <Measure
                    value={`${(o.durationMs / 1000).toFixed(1)} s`}
                    className="text-foreground"
                  />
                </span>
              ) : null}
              {o.fee !== null ? (
                <span>
                  fee{" "}
                  <Amount
                    unit="MINA"
                    value={formatMina(o.fee)}
                    className="text-foreground"
                  />
                </span>
              ) : null}
            </p>
          </li>
        ))}
      </ul>
      <Table
        className="table-fixed [&_td]:align-middle [&_td]:whitespace-nowrap"
        containerProps={{
          ...tableRegion("Recent jobs"),
          className: "hidden sm:block",
        }}
      >
        <TableHeader>
          <TableRow>
            {/* Fixed columns, so they don't move as "When" grows. */}
            <TableHead className="w-40">When</TableHead>
            <TableHead>Job</TableHead>
            <TableHead className="w-24 text-right">Took (s)</TableHead>
            <TableHead className="w-28 text-right">Fee (MINA)</TableHead>
            <TableHead className="w-36">Result</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {shown.map((o, i) => (
            <TableRow key={`${o.jobId}-${o.timeMs}-${i}`}>
              <TableCell>
                <TimeAgo at={o.timeMs} />
              </TableCell>
              <TableCell>{describeJob(o.summary, o.zkapp)}</TableCell>
              <TableCell className="text-right font-mono tabular-nums">
                {o.durationMs !== null ? (o.durationMs / 1000).toFixed(1) : "–"}
              </TableCell>
              <TableCell className="text-right font-mono tabular-nums">
                {formatMina(o.fee) ?? "–"}
              </TableCell>
              <TableCell>
                <JobResult job={o} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </PageSection>
  );
}

/** What became of a job: offered to the pool, not offered, or failed. */
function JobResult({ job: o }: { job: WorkerControl["recent"][number] }) {
  if (o.error) {
    return (
      <Signal variant="plain" tone="warn" title={o.error}>
        Failed
      </Signal>
    );
  }
  return o.offered ? (
    <Signal variant="plain" tone="ok">
      Offered
    </Signal>
  ) : (
    <Signal
      variant="plain"
      tone="neutral"
      title="Proved for a key or fee the worker no longer has"
    >
      Not offered
    </Signal>
  );
}

export function IncludedSection({
  included,
  tip,
}: {
  included: Polled<IncludedWork>;
  /** The best tip's height, to count confirmations. */
  tip: number | null;
}) {
  const { shown, hidden, toggle } = useShowAll(
    included.data?.entries ?? [],
    "newest",
  );
  return (
    <PageSection title="Included in blocks" aside={toggle}>
      <Loaded polled={included} what="the included-work log" rows={4}>
        {work => (
          <IncludedLog work={work} entries={shown} hidden={hidden} tip={tip} />
        )}
      </Loaded>
    </PageSection>
  );
}

function IncludedLog({
  work,
  entries,
  hidden,
  tip,
}: {
  work: IncludedWork;
  /** The rows to show, and how many more there are. */
  entries: readonly IncludedWork["entries"][number][];
  hidden: number;
  tip: number | null;
}) {
  if (work.entries.length === 0 && work.finalTotals.blocks === 0) {
    return (
      <EmptyState
        title="No block has bought this worker's work yet"
        description={<p>Blocks buy work in bursts, often hours apart.</p>}
      />
    );
  }
  const { confirmed, pending } = confirmationTotals(work, tip);
  const jobs = (t: IncludedWork["finalTotals"]) =>
    `${plural(t.bundles, "job")} in ${plural(t.blocks, "block")}`;
  return (
    <div className="flex flex-col gap-4">
      <StatRow
        label="Fees earned"
        // Side by side on a desktop; on a phone stacked, as Confirmed's
        // caption is too long for half the width.
        minItemWidth="12rem"
        items={[
          {
            label: "Confirmed",
            value: (
              <Amount
                unit="MINA"
                value={formatMina(confirmed.fees)}
                unitClassName="font-sans text-sm font-normal"
              />
            ),
            detail: `${jobs(confirmed)}, ${CONFIRMATIONS}+ blocks deep`,
          },
          {
            label: "Pending",
            value: (
              <Amount
                unit="MINA"
                value={formatMina(pending.fees)}
                unitClassName="font-sans text-sm font-normal"
              />
            ),
            detail:
              pending.bundles > 0
                ? `${jobs(pending)}, under ${CONFIRMATIONS} deep`
                : undefined,
          },
        ]}
      />
      {/* On a phone a block a row: the block and its state, then when,
          how deep, the jobs and the fees. */}
      <PhoneList
        label="Blocks that bought this worker's work"
        rows={entries.map(e => {
          const n = confirmations(e, tip);
          const state = confirmation(e, tip);
          return {
            key: `${e.height}-${e.stateHash}`,
            lead: (
              <BlockHeight
                height={e.height}
                url={explorerBlockUrl(work.network, e.height)}
              />
            ),
            end: <IncludedStatus entry={e} state={state} />,
            meta: [
              <TimeAgo key="t" at={e.timeMs} />,
              n === null ? null : state === "pending" ? (
                <>
                  <Fig n={n} /> of {CONFIRMATIONS} deep
                </>
              ) : (
                <>
                  <Fig n={n} /> deep
                </>
              ),
            ],
            // Jobs and fees on a line of their own: every row two lines.
            more: [
              <>
                <CountOf n={e.bundles} one="job" />
                {e.zkappBundles ? `, ${e.zkappBundles} zkApp` : null}
              </>,
              e.fees !== null ? (
                <Amount
                  unit="MINA"
                  value={formatMina(e.fees)}
                  className="text-foreground"
                />
              ) : null,
            ],
          };
        })}
      />
      <Table
        className="[&_td]:align-middle [&_td]:whitespace-nowrap"
        containerProps={{
          ...tableRegion("Blocks that bought this worker's work"),
          className: PHONE_HIDDEN,
        }}
      >
        <TableHeader>
          <TableRow>
            <TableHead>Block</TableHead>
            <TableHead>When</TableHead>
            <TableHead className="text-right">Confirmations</TableHead>
            <TableHead className="text-right">Jobs</TableHead>
            <TableHead className="text-right">Fees (MINA)</TableHead>
            <TableHead>Status</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {entries.map(e => {
            const url = explorerBlockUrl(work.network, e.height);
            const n = confirmations(e, tip);
            const state = confirmation(e, tip);
            return (
              <TableRow key={`${e.height}-${e.stateHash}`}>
                <TableCell className="font-mono tabular-nums">
                  {url ? (
                    <ExternalLink href={url} className={QUIET_EXTERNAL_LINK}>
                      {formatHeight(e.height)}
                    </ExternalLink>
                  ) : (
                    formatHeight(e.height)
                  )}
                </TableCell>
                <TableCell>
                  <TimeAgo at={e.timeMs} />
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {n === null ? (
                    "–"
                  ) : state === "pending" ? (
                    <>
                      <Fig n={n} />
                      <span className="font-sans text-muted-foreground">
                        {" "}
                        of {CONFIRMATIONS}
                      </span>
                    </>
                  ) : (
                    formatCount(n)
                  )}
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {formatCount(e.bundles)}
                  {e.zkappBundles ? (
                    // The figure in mono, the word in the text face.
                    <span className="ml-1.5 font-sans text-xs text-muted-foreground">
                      {e.zkappBundles} zkApp
                    </span>
                  ) : null}
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {formatMina(e.fees) ?? "–"}
                </TableCell>
                <TableCell>
                  <IncludedStatus entry={e} state={state} />
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      {hidden > 0 ? (
        <p className="text-sm text-muted-foreground">
          The newest {entries.length} of {formatCount(work.entries.length)}{" "}
          blocks; totals count all {formatCount(work.entries.length)}.
        </p>
      ) : null}
    </div>
  );
}

/** The SNARK work market as the node sees it: what its best chain bought
 * from every prover, and the proofs waiting in its pool. */
function Market({
  market,
  network,
  ourKey,
  ourFee,
  ownKeys,
}: {
  market: SnarkMarket;
  network: string | null;
  ourKey: string | null;
  ourFee: number | null;
  /** The keys the node counts as its own (the included-work log's). */
  ownKeys: readonly string[];
}) {
  const comparison = feeComparison(market, ourKey, ourFee, ownKeys);
  // The cheapest ten, and this worker's offer wherever it falls; the node
  // caps the list, and says how many provers there are in all.
  const ourOffer = market.offers.find(o => o.prover === ourKey);
  const cheapest = market.offers.slice(0, 10);
  const shownOffers =
    ourOffer && !cheapest.includes(ourOffer)
      ? [...cheapest, ourOffer]
      : cheapest;
  const notShown =
    marketProvers(market, ourKey === null ? ownKeys : [...ownKeys, ourKey])
      .total - shownOffers.length;
  return (
    <PageSection title="The SNARK market">
      <div className="grid items-start gap-6 lg:grid-cols-2">
        <MarketTable
          title="Bought by blocks"
          phone={
            market.purchases.length ? (
              <PhoneList
                label="Bought by blocks"
                rows={market.purchases.map(p => ({
                  key: `${p.height}-${p.timeMs}`,
                  lead: (
                    <BlockHeight
                      height={p.height}
                      url={explorerBlockUrl(network, p.height)}
                    />
                  ),
                  end: (
                    <>
                      <FeeRange lo={p.minFee} hi={p.maxFee} />
                      {/* The unit as every figure's: mono, muted. */}
                      <span className="font-mono text-muted-foreground">
                        {" "}
                        MINA
                      </span>
                    </>
                  ),
                  meta: [
                    <TimeAgo key="t" at={p.timeMs} />,
                    <CountOf key="j" n={p.bundles} one="job" />,
                    <CountOf key="p" n={p.provers} one="prover" />,
                  ],
                }))}
              />
            ) : undefined
          }
          footer={
            market.buyingBlocks
              ? `${formatCount(market.buyingBlocks)} of the last ${plural(market.blocks, "best-chain block")} bought work: ${plural(market.bundles, "job")} in all.`
              : `None of the last ${plural(market.blocks, "best-chain block")} bought work.`
          }
        >
          <TableHeader>
            <TableRow>
              <TableHead>Block</TableHead>
              <TableHead>When</TableHead>
              <TableHead className="text-right">Jobs</TableHead>
              <TableHead className="text-right">Provers</TableHead>
              <TableHead className="text-right">Fee (MINA)</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {market.purchases.length ? (
              market.purchases.map(p => {
                const url = explorerBlockUrl(network, p.height);
                return (
                  <TableRow key={`${p.height}-${p.timeMs}`}>
                    <TableCell className="font-mono tabular-nums">
                      {url ? (
                        <ExternalLink
                          href={url}
                          className={QUIET_EXTERNAL_LINK}
                        >
                          {formatHeight(p.height)}
                        </ExternalLink>
                      ) : (
                        formatHeight(p.height)
                      )}
                    </TableCell>
                    <TableCell>
                      <TimeAgo at={p.timeMs} />
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      {formatCount(p.bundles)}
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">
                      {formatCount(p.provers)}
                    </TableCell>
                    <TableCell className="text-right">
                      <FeeRange lo={p.minFee} hi={p.maxFee} />
                    </TableCell>
                  </TableRow>
                );
              })
            ) : (
              <TableRow>
                <TableCell colSpan={5} className="text-muted-foreground">
                  No purchases yet
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </MarketTable>
        <MarketTable
          title="Offers in the pool"
          footer={
            <>
              {comparison ?? "Blocks buy the cheapest proof of each job."}
              {notShown > 0
                ? ` ${plural(notShown, "more prover")} not shown.`
                : null}
            </>
          }
        >
          <TableHeader>
            <TableRow>
              <TableHead>Prover</TableHead>
              <TableHead className="text-right">Jobs</TableHead>
              <TableHead className="text-right">Fee (MINA)</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {shownOffers.length ? (
              shownOffers.map(o => (
                <TableRow key={o.prover}>
                  <TableCell>
                    <span className="inline-flex flex-wrap items-center gap-x-2">
                      <Hash value={o.prover} label="prover key" />
                      {o.prover === ourKey ? (
                        <span className="font-sans text-xs text-muted-foreground">
                          this worker
                        </span>
                      ) : null}
                    </span>
                  </TableCell>
                  <TableCell className="text-right font-mono tabular-nums">
                    {formatCount(o.bundles)}
                  </TableCell>
                  <TableCell className="text-right">
                    <FeeRange lo={o.minFee} hi={o.maxFee} />
                  </TableCell>
                </TableRow>
              ))
            ) : (
              <TableRow>
                <TableCell colSpan={3} className="text-muted-foreground">
                  None in the pool
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </MarketTable>
      </div>
    </PageSection>
  );
}

/** A block's height in a role table: to the explorer when the network has
 * one. */
function BlockHeight({ height, url }: { height: number; url: string | null }) {
  return (
    <span className="font-mono tabular-nums">
      {url ? (
        <ExternalLink href={url} className={QUIET_EXTERNAL_LINK}>
          {formatHeight(height)}
        </ExternalLink>
      ) : (
        formatHeight(height)
      )}
    </span>
  );
}

/** Where a block that bought this worker's work stands. */
function IncludedStatus({
  entry: e,
  state,
}: {
  entry: IncludedWork["entries"][number];
  state: ReturnType<typeof confirmation>;
}) {
  return (
    <Signal
      variant="plain"
      tone={confirmationTone[state]}
      title={
        e.status === "final"
          ? "Final: at or behind the node's root, 290 blocks deep"
          : e.status === "dropped"
            ? "Another block took its place in the best chain"
            : undefined
      }
    >
      {confirmationLabel[state]}
    </Signal>
  );
}

/** A count, muted at zero, in the figure's own colour otherwise. */
function Fig({ n }: { n: number }) {
  return (
    <span
      className={cn(
        "font-mono",
        n === 0 ? "text-muted-foreground" : "text-foreground",
      )}
    >
      {formatCount(n)}
    </span>
  );
}

/** One side of the market: a titled table with a line under it. */
function MarketTable({
  title,
  footer,
  phone,
  children,
}: {
  title: string;
  footer: React.ReactNode;
  /** Its rows on a phone, where the table would scroll its fee away. */
  phone?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <h3 className="text-[0.9375rem] font-semibold text-foreground">
        {title}
      </h3>
      {phone}
      <Table
        className="[&_td]:align-middle [&_td]:whitespace-nowrap"
        containerProps={{
          ...tableRegion(title),
          className: phone ? PHONE_HIDDEN : undefined,
        }}
      >
        {children}
      </Table>
      {/* Balanced, so a half-column note never leaves a figure alone on
          its last line. */}
      <p className="text-sm text-muted-foreground max-sm:text-pretty sm:text-balance">
        {footer}
      </p>
    </div>
  );
}

/** A fee, or the range of fees, in MINA: figures in mono, the word between
 * them in the text face. */
function FeeRange({ lo, hi }: { lo: number; hi: number }) {
  return (
    <span className="font-mono tabular-nums">
      {formatMina(lo)}
      {lo === hi ? null : (
        <>
          <span className="mx-1 font-sans text-muted-foreground">to</span>
          {formatMina(hi)}
        </>
      )}
    </span>
  );
}
