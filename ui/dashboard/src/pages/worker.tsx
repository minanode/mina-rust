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
  CardDescription,
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
import { MetricCard } from "@minanode/design-system/patterns/metric-card";
import { Signal } from "@minanode/design-system/patterns/signal";

import { Amount } from "@minanode/design-system/patterns/amount";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";

import {
  describeError,
  describeJob,
  explorerBlockUrl,
  feeInput,
  formatCount,
  formatDuration,
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

import { Code } from "../components/code";
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
        description={
          <>
            Proves the transaction SNARKs that blocks need, for a fee paid to
            your key when a block buys the work.{" "}
            <ExternalLink href={docsUrl("worker")}>
              SNARK worker docs
            </ExternalLink>
          </>
        }
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
        <IncludedSection included={included} tip={tip} market={synced} />
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
    return (
      <div className="lg:max-w-2xl">
        <ControlCard control={c} market={market} onChanged={onChanged} />
      </div>
    );
  }
  return (
    <EmptyState
      icon={<Cpu />}
      title="The SNARK worker is off on this node"
      description={
        <>
          <p>
            A worker proves the SNARK work that blocks buy, and the fees go to a
            key you choose. To run it, restart the node with{" "}
            <Code>--run-snarker &lt;public key&gt;</Code> and{" "}
            <Code>--snarker-fee &lt;MINA&gt;</Code>.
            {c.strategy ? (
              <>
                {" "}
                It would pick jobs{" "}
                {c.strategy === "Random" ? "at random" : "in order"} (
                <Code>
                  --snarker-strategy {c.strategy === "Random" ? "rand" : "seq"}
                </Code>
                ).
              </>
            ) : null}
          </p>
          <p className="mt-2">
            To start, stop and re-price it from this page instead, restart the
            node with <Code>--allow-worker-control</Code>. It changes where fees
            go, so keep the node&apos;s HTTP port reachable only by you.
          </p>
        </>
      }
    />
  );
}

/** The worker runs, but this page may not change it. */
function ControlOff() {
  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Lock aria-hidden className="size-4 text-brand-text" />
          Control is off on this node
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 text-sm leading-relaxed text-muted-foreground">
        <p>
          To stop or re-price the worker from this page, restart the node with{" "}
          <Code>--allow-worker-control</Code>. It changes where fees go, so keep
          the node&apos;s HTTP port reachable only by you.
        </p>
        <p>
          Without it, the worker runs as set at startup by{" "}
          <Code>--run-snarker</Code> and <Code>--snarker-fee</Code>.
        </p>
      </CardContent>
    </Card>
  );
}

function WorkerStatus({
  control: c,
  jobs,
}: {
  control: WorkerControl;
  /** Jobs at the tip, from the node's status. */
  jobs?: number | null;
}) {
  const view = workerView(c);
  const rows: { label: string; value: React.ReactNode; tone?: "brand" }[] = [
    {
      label: "State",
      value:
        view.label === "Working" && c.job ? (
          <span>
            Proving: {describeJob(c.job.summary, c.job.zkapp).toLowerCase()}
            {c.job.submittedMs ? (
              <>
                {", started "}
                <TimeAgo at={c.job.submittedMs} />
              </>
            ) : null}
          </span>
        ) : (
          view.detail
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
      value:
        c.accountExists === null
          ? "Not known yet"
          : c.accountExists
            ? "In the ledger"
            : "Not in the ledger yet",
    });
  }
  if (c.fee !== null) {
    rows.push({
      label: "Fee per job",
      value: `${formatMina(c.fee)} MINA`,
      tone: "brand",
    });
  }
  if (c.strategy) {
    rows.push({ label: "Picks jobs", value: strategyLabel(c.strategy) });
  }
  if (c.proofsMade !== null) {
    rows.push({
      label: "Proofs made",
      value: `${formatCount(c.proofsMade)} since the node started`,
    });
  }
  if (c.zkappJobs !== null) {
    rows.push({
      label: "Jobs the chain needs",
      value:
        jobs !== null && jobs !== undefined
          ? `${formatCount(jobs)}, ${formatCount(c.zkappJobs)} of them for zkApp transactions`
          : `${formatCount(c.zkappJobs)} for zkApp transactions`,
    });
  }
  return (
    <div className="flex flex-col gap-4">
      <KeyValueList
        title="Worker"
        rows={rows}
        footer="A job is one or two proofs that a block buys together, for one fee."
      />
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
    <Card className="gap-5">
      <CardHeader>
        <CardTitle>
          {running ? "Change the worker" : "The SNARK worker is off"}
        </CardTitle>
        <CardDescription>
          {running
            ? "The node keeps this setting in its work directory, so a restart brings it back."
            : "Start it here: it proves the SNARK work that blocks buy, for the fee you set, paid to the key you give. The node keeps the setting, so a restart brings it back."}
          {c.accountCreationFee !== null ? (
            <>
              {" "}
              A key that has no account yet needs a fee of at least{" "}
              {formatMina(c.accountCreationFee)} MINA.
            </>
          ) : null}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form
          noValidate
          onSubmit={submit}
          className="flex flex-col gap-4"
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
            <p id={feeHintId} className="text-sm text-muted-foreground">
              {!running && lowestOffer !== null ? (
                <>
                  Lowest offer in the pool now: {formatMina(lowestOffer)}{" "}
                  MINA.{" "}
                </>
              ) : null}
              What a block pays for each job it buys from this worker: one or
              two proofs.
              {!running && c.strategy ? (
                <>
                  {" "}
                  It picks jobs{" "}
                  {c.strategy === "Random" ? "at random" : "in order"} (
                  <Code>
                    --snarker-strategy{" "}
                    {c.strategy === "Random" ? "rand" : "seq"}
                  </Code>
                  , set when the node starts).
                </>
              ) : null}
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
        <div aria-live="polite" className="mt-4 empty:hidden">
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

function RecentJobs({ control: c }: { control: WorkerControl }) {
  const { shown, toggle } = useShowAll(c.recent, "newest");
  return (
    <PageSection
      title="Recent jobs"
      description="The worker's latest outcomes, newest first."
      aside={toggle}
    >
      <Table
        className="[&_td]:align-middle [&_td]:whitespace-nowrap"
        containerProps={tableRegion("Recent jobs")}
      >
        <TableHeader>
          <TableRow>
            <TableHead>When</TableHead>
            <TableHead>Job</TableHead>
            <TableHead className="text-right">Took</TableHead>
            <TableHead className="text-right">Fee</TableHead>
            <TableHead>Result</TableHead>
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
                {o.durationMs !== null ? formatDuration(o.durationMs) : "–"}
              </TableCell>
              <TableCell className="text-right">
                <Amount unit="MINA" value={formatMina(o.fee)} />
              </TableCell>
              <TableCell>
                {o.error ? (
                  <Signal tone="warn" title={o.error}>
                    Failed
                  </Signal>
                ) : o.offered ? (
                  <Signal tone="ok">Offered</Signal>
                ) : (
                  <Signal
                    tone="neutral"
                    title="Proved for a key or fee the worker no longer has"
                  >
                    Not offered
                  </Signal>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </PageSection>
  );
}

function IncludedSection({
  included,
  tip,
  market,
}: {
  included: Polled<IncludedWork>;
  /** The best tip's height, to count confirmations. */
  tip: number | null;
  /** The market section follows. */
  market: boolean;
}) {
  const { shown, hidden, toggle } = useShowAll(
    included.data?.entries ?? [],
    "newest",
  );
  return (
    <PageSection
      title="Included in blocks"
      description={`Best-chain blocks that bought this worker's work, newest first. It is confirmed once ${CONFIRMATIONS} blocks are on top of its block, Mina's recommended wait for a payment; until then a reorg could still drop it.`}
      aside={toggle}
    >
      <Loaded polled={included} what="the included-work log" rows={4}>
        {work => (
          <IncludedLog
            work={work}
            entries={shown}
            hidden={hidden}
            tip={tip}
            market={market && work.market !== null}
          />
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
  market,
}: {
  work: IncludedWork;
  /** The rows to show, and how many more there are. */
  entries: readonly IncludedWork["entries"][number][];
  hidden: number;
  tip: number | null;
  market: boolean;
}) {
  if (work.entries.length === 0 && work.finalTotals.blocks === 0) {
    return (
      <EmptyState
        title="No block has bought this worker's work yet"
        description={
          <p>
            Blocks buy work only when the chain needs proofs, often many jobs at
            once and then none for hours, so a new worker can wait a while.
            {market
              ? " The market below shows when blocks last bought and at what fees."
              : null}
          </p>
        }
      />
    );
  }
  const { confirmed, pending } = confirmationTotals(work, tip);
  const jobs = (t: IncludedWork["finalTotals"]) =>
    `${plural(t.bundles, "job")} in ${plural(t.blocks, "block")}`;
  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <MetricCard
          size="sm"
          value={`${formatMina(confirmed.fees)} MINA`}
          label="Confirmed"
          detail={`${jobs(confirmed)}, each with ${CONFIRMATIONS} or more blocks on top, since the log began.`}
        />
        <MetricCard
          size="sm"
          value={`${formatMina(pending.fees)} MINA`}
          label="Pending"
          detail={`${jobs(pending)} with fewer than ${CONFIRMATIONS} blocks on top.`}
        />
      </div>
      <Table
        className="[&_td]:align-middle [&_td]:whitespace-nowrap"
        containerProps={tableRegion("Blocks that bought this worker's work")}
      >
        <TableHeader>
          <TableRow>
            <TableHead>Block</TableHead>
            <TableHead>When</TableHead>
            <TableHead className="text-right">Confirmations</TableHead>
            <TableHead className="text-right">Jobs</TableHead>
            <TableHead className="text-right">Fees</TableHead>
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
                <TableCell className="font-mono">
                  {url ? (
                    <ExternalLink href={url}>
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
                  {n === null
                    ? "–"
                    : state === "pending"
                      ? `${formatCount(n)} of ${CONFIRMATIONS}`
                      : formatCount(n)}
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {formatCount(e.bundles)}
                  {e.zkappBundles ? (
                    <span className="text-muted-foreground">
                      {" "}
                      ({e.zkappBundles} zkApp)
                    </span>
                  ) : null}
                </TableCell>
                <TableCell className="text-right">
                  <Amount unit="MINA" value={formatMina(e.fees)} />
                </TableCell>
                <TableCell>
                  <Signal
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
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      {hidden > 0 ? (
        <p className="text-sm text-muted-foreground">
          The newest {entries.length} of {formatCount(work.entries.length)}{" "}
          blocks; the totals above count them all.
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
  const fees = (lo: number, hi: number) =>
    lo === hi
      ? `${formatMina(lo)} MINA`
      : `${formatMina(lo)} to ${formatMina(hi)} MINA`;
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
    <PageSection
      title="The SNARK market"
      description="What this node's best chain bought from every prover, and the proofs waiting in its pool, to compare fees with."
    >
      <div className="grid items-start gap-6 lg:grid-cols-2">
        <KeyValueList
          title="Bought by blocks"
          rows={
            market.purchases.length
              ? market.purchases.map(p => {
                  const url = explorerBlockUrl(network, p.height);
                  return {
                    label: (
                      <span className="inline-flex flex-wrap gap-x-2">
                        {url ? (
                          <ExternalLink href={url}>
                            {formatHeight(p.height)}
                          </ExternalLink>
                        ) : (
                          formatHeight(p.height)
                        )}
                        <TimeAgo at={p.timeMs} />
                      </span>
                    ),
                    value: `${plural(p.bundles, "job")} from ${plural(p.provers, "prover")}, ${fees(p.minFee, p.maxFee)}`,
                  };
                })
              : [{ label: "Latest purchases", value: "None" }]
          }
          footer={
            market.buyingBlocks
              ? `${formatCount(market.buyingBlocks)} of the last ${plural(market.blocks, "best-chain block")} bought work: ${plural(market.bundles, "job")} in all.`
              : `None of the last ${plural(market.blocks, "best-chain block")} bought work.`
          }
        />
        <KeyValueList
          title="Offers in the pool"
          rows={
            shownOffers.length
              ? shownOffers.map(o => ({
                  label: (
                    <span className="inline-flex flex-wrap items-center gap-x-2">
                      <Hash value={o.prover} label="prover key" />
                      {o.prover === ourKey ? (
                        <span className="font-sans text-xs text-brand-text">
                          this worker
                        </span>
                      ) : null}
                    </span>
                  ),
                  value: `${plural(o.bundles, "job")} at ${fees(o.minFee, o.maxFee)}`,
                  tone: o.prover === ourKey ? ("brand" as const) : undefined,
                }))
              : [{ label: "Offers", value: "None in the pool" }]
          }
          footer={
            <>
              {comparison ?? "Blocks buy the cheapest proof of each job."}
              {notShown > 0
                ? ` ${plural(notShown, "more prover")} not shown.`
                : null}
            </>
          }
        />
      </div>
    </PageSection>
  );
}
