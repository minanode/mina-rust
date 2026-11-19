/**
 * The Archive page, on a node that feeds an OCaml archive process: whether
 * the feed is up to date, the archive's own counts of its database (when the
 * node reads its metrics), the outbox's figures and gaps, and replay of
 * retained blocks. On any other node the route says there is no such page.
 */

import * as React from "react";
import { CircleCheck, Search, TriangleAlert, Undo2 } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";
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
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import { Signal } from "@minanode/design-system/patterns/signal";
import { StatRow } from "@minanode/design-system/patterns/stat-row";
import { useNow } from "@minanode/design-system/lib/time";

import {
  archiveDatabaseView,
  archiveEntryLabel,
  archiveFeedView,
  archiveGapLabel,
  archiveLookup,
  describeError,
  formatBytes,
  formatCount,
  formatHeight,
  kindOf,
  NodeError,
  plural,
  utcDateTime,
  type ArchiveBlockRef,
  type ArchiveDatabase,
  type ArchiveEntry,
  type ArchiveFeedView,
  type ArchiveGap,
  type ArchiveQuery,
  type ArchiveStatus,
} from "@minanode/node-client";
import {
  feedsArchive,
  useArchiveBlocks,
  useArchiveStatus,
  useNodeClient,
  useNodeStatus,
  useSyncView,
} from "@minanode/node-client/react";

import { Code } from "../components/code";
import { PHONE_HIDDEN, PhoneList } from "../components/phone-list";
import { useShowAll } from "../components/show-all";
import { Loaded, PolledAge } from "../components/states";
import { docsUrl } from "../lib/docs";
import { NotFoundPage } from "./not-found";

const POLL_MS = 5_000;

/** Recently delivered blocks the page lists, each with a replay. */
export const RECENT_DELIVERED = 20;

export function ArchivePage({ path = "/archive-node" }: { path?: string }) {
  const archive = useArchiveStatus(POLL_MS);
  const now = useNow();
  const sync = useSyncView(now);
  const { status } = useNodeStatus();
  // A node that doesn't feed an archive has no Archive page.
  if (!feedsArchive(archive) && kindOf(archive.error) === "absent") {
    return <NotFoundPage path={path} />;
  }
  const tip = status.data?.bestTip?.height ?? null;
  const view = archive.data
    ? archiveFeedView(archive.data, {
        synced: sync.key === "Synced",
        bestTipHeight: tip,
      })
    : null;
  return (
    <>
      <PageHeader
        eyebrow="Roles"
        title="Archive"
        aside={
          <>
            {view ? (
              <Signal
                tone={view.tone}
                pulse={view.key === "CatchingUp"}
                className="text-sm"
                data-archive-feed={view.key}
              >
                {view.label}
              </Signal>
            ) : null}
            <PolledAge polled={archive} staleAfterMs={POLL_MS * 4} />
            {/* The one link to the explanation. */}
            <ExternalLink href={docsUrl("archive")} className="text-sm">
              Archive docs
            </ExternalLink>
          </>
        }
      />
      <Loaded polled={archive} what="the archive feed" rows={6}>
        {s => (
          <ArchiveFeed
            status={s}
            view={view!}
            tip={tip}
            onChanged={archive.refresh}
          />
        )}
      </Loaded>
    </>
  );
}

function ArchiveFeed({
  status: s,
  view,
  tip,
  onChanged,
}: {
  status: ArchiveStatus;
  view: ArchiveFeedView;
  tip: number | null;
  onChanged: () => void;
}) {
  return (
    <>
      <StatRow
        label="Feed at a glance"
        items={[
          { label: "Best tip", value: formatHeight(tip) },
          {
            label: "Last delivered",
            value: formatHeight(view.deliveredHeight),
            detail:
              s.lastSuccessMs !== null ? (
                <TimeAgo at={s.lastSuccessMs} />
              ) : null,
          },
          {
            label: "Behind",
            value: view.behind === null ? "–" : plural(view.behind, "block"),
          },
          { label: "Waiting", value: formatCount(s.pendingEntries) },
        ]}
      />
      {view.key === "Failing" ? <Failing status={s} /> : null}
      <div className="grid items-start gap-6 lg:grid-cols-2">
        <div className="flex flex-col gap-6">
          <DatabaseCheck status={s} />
          <FeedValues status={s} />
        </div>
        <Retained status={s} onChanged={onChanged} />
      </div>
      <Gaps status={s} />
      <ReplaySection onChanged={onChanged} />
      <RecentDelivered onChanged={onChanged} />
    </>
  );
}

function BlockValue({ block }: { block: ArchiveBlockRef | null }) {
  if (!block) return <>–</>;
  return (
    <span className="flex flex-wrap items-center justify-end gap-x-2">
      <span>{formatHeight(block.height)}</span>
      <Hash value={block.stateHash} label="state hash" />
    </span>
  );
}

function Failing({ status: s }: { status: ArchiveStatus }) {
  return (
    <Alert variant="destructive" data-archive-failing>
      <TriangleAlert aria-hidden />
      <AlertTitle>
        Delivery failing
        {s.failingSinceMs !== null ? (
          <>
            {" "}
            since {utcDateTime(s.failingSinceMs)} (
            <TimeAgo at={s.failingSinceMs} />)
          </>
        ) : null}
      </AlertTitle>
      <AlertDescription>
        {s.lastError ? (
          <p className="font-mono break-words">{s.lastError}</p>
        ) : null}
        <p>
          {plural(s.consecutiveFailures, "attempt")} failed
          {s.stuckOn ? (
            <>
              {" "}
              on block {formatHeight(s.stuckOn.height)}{" "}
              <Hash value={s.stuckOn.stateHash} label="state hash" />
            </>
          ) : null}
          .
        </p>
      </AlertDescription>
    </Alert>
  );
}

function FeedValues({ status: s }: { status: ArchiveStatus }) {
  return (
    <KeyValueList
      title="Feed"
      rows={[
        {
          label: "Archive address",
          value: s.archiveAddress ?? "–",
        },
        {
          label: "Last delivered",
          value: <BlockValue block={s.lastDelivered} />,
          note:
            s.lastSuccessMs !== null ? (
              <>
                acknowledged <TimeAgo at={s.lastSuccessMs} />
              </>
            ) : undefined,
        },
        {
          label: "Highest accepted",
          value: formatHeight(s.highestAcceptedHeight || null),
        },
        {
          label: "Waiting",
          value: formatCount(s.pendingEntries),
          note: s.pendingEntries > 0 ? formatBytes(s.pendingBytes) : undefined,
        },
        {
          label: "Feed started at",
          value: <BlockValue block={s.feedStartedAt} />,
        },
        {
          label: "Delivered since start",
          value: formatCount(s.deliveredCount),
        },
      ]}
    />
  );
}

/** The stored tip against the highest delivered block, in words. */
function tipLag(lag: number | null): string | undefined {
  if (lag === null) return undefined;
  if (lag === 0) return "same as last delivered";
  return lag > 0
    ? `${plural(lag, "block")} below last delivered`
    : `${plural(-lag, "block")} above last delivered`;
}

/** What the operator should do about the verdict; nothing when the
 * database is complete or not yet checked, so the card then has no
 * footer. */
function databaseAction(
  db: ArchiveDatabase,
  s: ArchiveStatus,
): React.ReactNode {
  if (!db.configured) {
    return (
      <p data-archive-database-action="configure">
        Set <Code>MINA_ARCHIVE_METRICS_ADDRESS</Code> to the archive's{" "}
        <Code>--metrics-port</Code> URL.
      </p>
    );
  }
  switch (db.verdict) {
    case "incomplete":
      return (
        <p data-archive-database-action="resend" className="text-foreground">
          <TriangleAlert
            aria-hidden
            className="mr-1.5 inline size-4 align-[-0.2em] text-destructive"
          />
          {s.oldestRetained && s.newestRetained
            ? `Resend all retained to refill heights ${formatHeight(s.oldestRetained.height)} to ${formatHeight(s.newestRetained.height)}.`
            : "Resend all retained to refill the heights this node keeps."}{" "}
          Older heights need a precomputed-block import.
        </p>
      );
    case "behind":
      return (
        <p data-archive-database-action="behind" className="text-foreground">
          <TriangleAlert
            aria-hidden
            className="mr-1.5 inline size-4 align-[-0.2em] text-status-pending"
          />
          Check the archive's log and its database connection.
        </p>
      );
    case "unavailable":
      return (
        <p data-archive-database-action="unavailable">
          Check the archive runs with <Code>--metrics-port</Code> at{" "}
          <Code>MINA_ARCHIVE_METRICS_ADDRESS</Code>.
        </p>
      );
    default:
      return null;
  }
}

function DatabaseCheck({ status: s }: { status: ArchiveStatus }) {
  const db = s.database;
  const view = archiveDatabaseView(db);
  const rows = db.configured
    ? [
        {
          label: "Highest stored",
          value: formatHeight(db.maxBlockHeight),
          note: tipLag(db.storedTipLag),
        },
        {
          label: "Missing heights",
          value: formatCount(db.missingBlocks),
          note: "recent window",
        },
        {
          label: "Blocks without a parent",
          value: formatCount(db.unparentedBlocks),
        },
        {
          label: "Checked",
          value:
            db.lastSuccessMs !== null ? <TimeAgo at={db.lastSuccessMs} /> : "–",
        },
        ...(db.verdict === "unavailable" && db.lastError
          ? [{ label: "Last error", value: db.lastError, wide: true }]
          : []),
        { label: "Metrics", value: db.metricsUrl ?? "–", wide: true },
      ]
    : [];
  return (
    <KeyValueList
      title="Archive database"
      data-archive-database={db.verdict}
      aside={
        <Signal tone={view.tone} className="text-sm">
          {view.label}
        </Signal>
      }
      rows={rows}
      footer={databaseAction(db, s)}
    />
  );
}

type Action =
  | { kind: "idle" }
  | { kind: "sending"; what: string }
  | { kind: "done"; message: string }
  | { kind: "failed"; message: string; origin: boolean };

function failed(error: unknown): Action {
  const message = describeError(error);
  return {
    kind: "failed",
    message,
    origin:
      error instanceof NodeError &&
      error.status === 403 &&
      /origin/i.test(message),
  };
}

function ActionResult({ action }: { action: Action }) {
  return (
    <div aria-live="polite" className="empty:hidden">
      {action.kind === "done" ? (
        <Alert variant="brand" data-archive-result="done">
          <CircleCheck aria-hidden />
          <AlertTitle>Requeued</AlertTitle>
          <AlertDescription>
            <p>{action.message}</p>
          </AlertDescription>
        </Alert>
      ) : null}
      {action.kind === "failed" ? (
        <Alert variant="destructive" data-archive-result="failed">
          <TriangleAlert aria-hidden />
          <AlertTitle>The node refused the replay</AlertTitle>
          <AlertDescription>
            <p>{action.message}</p>
            {action.origin ? (
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
  );
}

function Retained({
  status: s,
  onChanged,
}: {
  status: ArchiveStatus;
  onChanged: () => void;
}) {
  const client = useNodeClient();
  const [confirming, setConfirming] = React.useState(false);
  const [action, setAction] = React.useState<Action>({ kind: "idle" });
  const sending = action.kind === "sending";
  const window =
    s.oldestRetained && s.newestRetained
      ? `${formatHeight(s.oldestRetained.height)} to ${formatHeight(s.newestRetained.height)}`
      : "–";
  async function resendAll() {
    setAction({ kind: "sending", what: "all" });
    try {
      const done = await client.replayArchiveAll();
      setAction({
        kind: "done",
        message: `${plural(done.requeued, "block")} queued for the archive again.`,
      });
      setConfirming(false);
      onChanged();
    } catch (error) {
      setAction(failed(error));
    }
  }
  return (
    <div className="flex flex-col gap-4">
      <KeyValueList
        title="Retained for replay"
        rows={[
          { label: "Heights", value: window },
          { label: "Blocks", value: formatCount(s.retainedEntries) },
          { label: "Size", value: formatBytes(s.retainedBytes) },
          {
            label: "Window",
            value: plural(s.retainBlocks, "height"),
            note: "below the highest accepted",
          },
        ]}
        footer={
          <div className="flex flex-col gap-3 py-3">
            {confirming ? (
              <InlineConfirm
                label="Confirm resending every retained block"
                message={`Send all ${plural(s.retainedEntries, "retained block")} (${formatBytes(s.retainedBytes)}) to the archive again? New blocks wait until they are through.`}
                confirmLabel="Resend all"
                busyLabel={sending ? "Resending…" : undefined}
                busy={sending}
                onConfirm={() => void resendAll()}
                onCancel={() => setConfirming(false)}
              />
            ) : (
              <Button
                type="button"
                variant="outline"
                className="w-fit"
                disabled={s.retainedEntries === 0 || sending}
                onClick={() => {
                  setAction({ kind: "idle" });
                  setConfirming(true);
                }}
              >
                <Undo2 aria-hidden />
                Resend all retained
              </Button>
            )}
          </div>
        }
      />
      <ActionResult action={action} />
    </div>
  );
}

function gapBlock(g: ArchiveGap): string | null {
  return g.stateHash ?? g.parentHash ?? null;
}

function gapHeights(g: ArchiveGap): string {
  return g.fromHeight === g.toHeight
    ? formatHeight(g.fromHeight)
    : `${formatHeight(g.fromHeight)} to ${formatHeight(g.toHeight)}`;
}

function Gaps({ status: s }: { status: ArchiveStatus }) {
  // Newest first.
  const gaps = [...s.recentGaps].reverse();
  return (
    <PageSection
      title="Gaps since the feed started"
      aside={
        s.gapsTotal > 0 ? (
          <span className="font-mono text-sm text-muted-foreground tabular-nums">
            {formatCount(s.gapsTotal)}
          </span>
        ) : null
      }
    >
      {s.gapsTotal === 0 ? (
        <EmptyState icon={<CircleCheck />} title="No gaps" />
      ) : (
        <>
          <PhoneList
            label="Gaps"
            rows={gaps.map((g, i) => ({
              key: `${g.kind}-${g.fromHeight}-${i}`,
              lead: archiveGapLabel(g.kind),
              end: <span className="font-mono">{gapHeights(g)}</span>,
              meta: [
                gapBlock(g) ? (
                  <Hash value={gapBlock(g)!} label="state hash" />
                ) : null,
                g.detectedMs !== null ? <TimeAgo at={g.detectedMs} /> : null,
              ],
            }))}
          />
          <Table
            className="[&_td]:align-middle"
            containerProps={{ ...tableRegion("Gaps"), className: PHONE_HIDDEN }}
          >
            <TableHeader>
              <TableRow>
                <TableHead>Kind</TableHead>
                <TableHead>Heights</TableHead>
                <TableHead>Block</TableHead>
                <TableHead className="w-36">Detected</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {gaps.map((g, i) => (
                <TableRow key={`${g.kind}-${g.fromHeight}-${i}`}>
                  <TableCell>{archiveGapLabel(g.kind)}</TableCell>
                  <TableCell className="font-mono tabular-nums">
                    {gapHeights(g)}
                  </TableCell>
                  <TableCell>
                    {gapBlock(g) ? (
                      <Hash value={gapBlock(g)!} label="state hash" />
                    ) : (
                      "–"
                    )}
                  </TableCell>
                  <TableCell>
                    {g.detectedMs !== null ? (
                      <TimeAgo at={g.detectedMs} />
                    ) : (
                      "–"
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          {s.gapsTotal > gaps.length ? (
            <p className="text-sm text-muted-foreground">
              The newest {formatCount(gaps.length)} of{" "}
              {formatCount(s.gapsTotal)}.
            </p>
          ) : null}
        </>
      )}
    </PageSection>
  );
}

/** One block's replay: the confirm, the request and its outcome. */
function useReplay(onChanged: () => void) {
  const client = useNodeClient();
  const [confirming, setConfirming] = React.useState<string | null>(null);
  const [action, setAction] = React.useState<Action>({ kind: "idle" });
  async function replay(entry: ArchiveEntry) {
    setAction({ kind: "sending", what: entry.stateHash });
    try {
      const done = await client.replayArchiveBlock(entry.stateHash);
      setAction({
        kind: "done",
        message:
          done.result === "already_pending"
            ? `Block ${formatHeight(entry.height)} is already ${done.inFlight ? "being sent" : "waiting"}.`
            : `Block ${formatHeight(entry.height)} is queued again, ahead of newer waiting blocks.`,
      });
      setConfirming(null);
      onChanged();
    } catch (error) {
      setAction(failed(error));
    }
  }
  return {
    action,
    confirming,
    ask: (hash: string) => {
      setAction({ kind: "idle" });
      setConfirming(hash);
    },
    cancel: () => setConfirming(null),
    /** Forget the confirm and the last outcome: a new lookup. */
    reset: () => {
      setConfirming(null);
      setAction({ kind: "idle" });
    },
    replay,
  };
}

type Replay = ReturnType<typeof useReplay>;

function StateSignal({ entry }: { entry: ArchiveEntry }) {
  return (
    <Signal
      variant="plain"
      tone={
        entry.state === "retained"
          ? "ok"
          : entry.state === "in_flight"
            ? "info"
            : "neutral"
      }
    >
      {archiveEntryLabel(entry.state)}
    </Signal>
  );
}

function ReplayAction({
  entry,
  replay,
}: {
  entry: ArchiveEntry;
  replay: Replay;
}) {
  if (entry.state !== "retained") return null;
  const busy =
    replay.action.kind === "sending" && replay.action.what === entry.stateHash;
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      disabled={busy || replay.confirming === entry.stateHash}
      onClick={() => replay.ask(entry.stateHash)}
      aria-label={`Replay block ${formatHeight(entry.height)}`}
    >
      <Undo2 aria-hidden />
      Replay
    </Button>
  );
}

function ReplayConfirm({
  entries,
  replay,
}: {
  entries: readonly ArchiveEntry[];
  replay: Replay;
}) {
  const entry = entries.find(e => e.stateHash === replay.confirming);
  if (!entry) return null;
  const sending = replay.action.kind === "sending";
  return (
    <InlineConfirm
      label={`Confirm replaying block ${formatHeight(entry.height)}`}
      message={`Send block ${formatHeight(entry.height)} to the archive again?`}
      confirmLabel="Replay"
      busyLabel={sending ? "Replaying…" : undefined}
      busy={sending}
      onConfirm={() => void replay.replay(entry)}
      onCancel={replay.cancel}
    />
  );
}

function Entries({
  label,
  entries,
  replay,
}: {
  label: string;
  entries: readonly ArchiveEntry[];
  replay: Replay;
}) {
  return (
    <>
      <PhoneList
        label={label}
        rows={entries.map(e => ({
          key: `${e.seq}`,
          lead: <span className="font-mono">{formatHeight(e.height)}</span>,
          end: <ReplayAction entry={e} replay={replay} />,
          meta: [
            <Hash key="hash" value={e.stateHash} label="state hash" />,
            <StateSignal key="state" entry={e} />,
          ],
        }))}
      />
      <Table
        className="[&_td]:align-middle"
        containerProps={{ ...tableRegion(label), className: PHONE_HIDDEN }}
      >
        <TableHeader>
          <TableRow>
            <TableHead className="w-32">Height</TableHead>
            <TableHead>State hash</TableHead>
            <TableHead className="w-32">State</TableHead>
            <TableHead className="w-28 text-right">
              <span className="sr-only">Action</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {entries.map(e => (
            <TableRow key={e.seq}>
              <TableCell className="font-mono tabular-nums">
                {formatHeight(e.height)}
              </TableCell>
              <TableCell>
                <Hash value={e.stateHash} label="state hash" />
              </TableCell>
              <TableCell>
                <StateSignal entry={e} />
              </TableCell>
              <TableCell className="text-right">
                <ReplayAction entry={e} replay={replay} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <ReplayConfirm entries={entries} replay={replay} />
    </>
  );
}

function ReplaySection({ onChanged }: { onChanged: () => void }) {
  const [input, setInput] = React.useState("");
  const [query, setQuery] = React.useState<ArchiveQuery | null>(null);
  const [inputError, setInputError] = React.useState<string | null>(null);
  const found = useArchiveBlocks(query, POLL_MS);
  const replay = useReplay(() => {
    onChanged();
    found.refresh();
  });
  const inputId = React.useId();
  const errorId = React.useId();

  function submit(event: React.FormEvent) {
    event.preventDefault();
    const lookup = archiveLookup(input);
    if (lookup.kind === "invalid") {
      setInputError(lookup.message);
      setQuery(null);
      return;
    }
    setInputError(null);
    replay.reset();
    setQuery(
      lookup.kind === "height"
        ? { height: lookup.height, limit: 20 }
        : { stateHash: lookup.stateHash, limit: 20 },
    );
  }

  return (
    <PageSection title="Replay a block">
      <form
        noValidate
        onSubmit={submit}
        className="flex max-w-3xl flex-col gap-2"
        role="search"
        aria-label="Find a block to replay"
      >
        <Label htmlFor={inputId}>Height or state hash</Label>
        <div className="flex flex-wrap gap-2">
          <Input
            id={inputId}
            value={input}
            onChange={e => setInput(e.target.value)}
            placeholder="Height or 3N…"
            autoComplete="off"
            spellCheck={false}
            className="min-w-0 flex-1 font-mono"
            aria-invalid={inputError !== null || undefined}
            aria-describedby={inputError ? errorId : undefined}
          />
          <Button type="submit" variant="outline">
            <Search aria-hidden />
            Find
          </Button>
        </div>
        {inputError ? (
          <p id={errorId} className="text-sm text-destructive">
            {inputError}
          </p>
        ) : null}
      </form>
      {query ? (
        <Loaded polled={found} what="the matching blocks" rows={2}>
          {list =>
            list.entries.length > 0 ? (
              <Entries
                label="Matching blocks"
                entries={list.entries}
                replay={replay}
              />
            ) : (
              <NotRetained
                query={query}
                oldest={list.oldestRetained}
                newest={list.newestRetained}
              />
            )
          }
        </Loaded>
      ) : null}
      <ActionResult action={replay.action} />
    </PageSection>
  );
}

function NotRetained({
  query,
  oldest,
  newest,
}: {
  query: ArchiveQuery;
  oldest: ArchiveBlockRef | null;
  newest: ArchiveBlockRef | null;
}) {
  const title =
    query.height !== undefined
      ? `Height ${formatHeight(query.height)} isn't retained by this node`
      : "That block isn't retained by this node";
  return (
    <EmptyState
      data-archive-not-retained
      tone="caution"
      icon={<TriangleAlert />}
      title={title}
      description={
        <p>
          {oldest && newest
            ? `It keeps heights ${formatHeight(oldest.height)} to ${formatHeight(newest.height)}.`
            : "It keeps no delivered blocks now."}{" "}
          Older blocks need a precomputed-block import, described in Archive
          docs.
        </p>
      }
    />
  );
}

function RecentDelivered({ onChanged }: { onChanged: () => void }) {
  const recent = useArchiveBlocks(
    { state: "retained", limit: RECENT_DELIVERED },
    POLL_MS,
  );
  const replay = useReplay(() => {
    onChanged();
    recent.refresh();
  });
  const { shown, toggle } = useShowAll(recent.data?.entries ?? [], "newest");
  return (
    <PageSection title="Recently delivered" aside={toggle}>
      <Loaded polled={recent} what="the delivered blocks" rows={4}>
        {list =>
          list.entries.length > 0 ? (
            <Entries
              label="Recently delivered blocks"
              entries={shown}
              replay={replay}
            />
          ) : (
            <EmptyState
              icon={<CircleCheck />}
              title="No delivered blocks retained"
            />
          )
        }
      </Loaded>
      <ActionResult action={replay.action} />
    </PageSection>
  );
}
