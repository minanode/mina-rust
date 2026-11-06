/** The boot's sync graphic: a bar a ledger, then the frontier's blocks as a
 * strip of cells that fill as they are fetched and applied. */

import { CircleCheck } from "lucide-react";

import { cn } from "@minanode/design-system/lib/utils";
import { formatCount } from "@minanode/node-client";

import type { BootSnapshot } from "../boot/boot";
import {
  blockStrip,
  ledgerRows,
  stripSentence,
  type BlockStrip,
  type CellState,
  type LedgerRow,
} from "../boot/progress";

/** A count in words: one Inter line, its figures in tabular Inter. Mono
 * figures inside a sentence would set a comma a full cell wide ("31, 758")
 * and a slashed zero beside Inter digits. */
export function Figures({ text }: { text: string }) {
  return <span className="tabular-nums">{text}</span>;
}

function LedgerBar({ row }: { row: LedgerRow }) {
  const done = row.status === "done";
  return (
    <li
      className="flex flex-col gap-1.5"
      data-ledger={row.key}
      data-status={row.status}
    >
      {/* Label and figure on one line where they fit; a long count wraps
          under its label on a phone. */}
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5">
        <span
          className={cn(
            "flex items-center gap-1.5 text-[0.8125rem]",
            row.status === "waiting"
              ? "text-muted-foreground"
              : "text-foreground",
          )}
        >
          {done ? (
            <CircleCheck aria-hidden className="size-3.5 text-status-live" />
          ) : null}
          {row.label}
        </span>
        <span className="text-xs text-muted-foreground tabular-nums">
          <Figures
            text={
              row.status === "syncing" && row.percent !== null
                ? `${row.text}, ${row.percent}%`
                : row.text
            }
          />
        </span>
      </div>
      {/* A ledger still waiting shows its word, not an empty track. */}
      {row.status === "waiting" ? null : (
        <div
          role="progressbar"
          aria-label={row.label}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={row.percent ?? undefined}
          aria-valuetext={row.text}
          className="h-1.5 overflow-hidden rounded-full bg-muted"
        >
          <div
            className={cn(
              "h-full rounded-full transition-[width] duration-500",
              done ? "bg-status-live" : "bg-status-devnet",
              row.status === "syncing" &&
                row.percent === null &&
                "w-1/4 opacity-60 motion-safe:animate-pulse",
            )}
            style={
              row.percent !== null ? { width: `${row.percent}%` } : undefined
            }
          />
        </div>
      )}
    </li>
  );
}

const CELL_CLASS: Record<CellState, string> = {
  // A cell to come stays visible against the card.
  missing: "bg-input/70",
  fetching: "bg-muted ring-1 ring-inset ring-status-devnet/70",
  fetched: "bg-status-devnet/45",
  applying: "bg-status-devnet motion-safe:animate-pulse",
  applied: "bg-status-live",
  failed: "bg-destructive",
};

function Legend({ className, label }: { className: string; label: string }) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <span aria-hidden className={cn("size-2.5 rounded-[2px]", className)} />
      {label}
    </span>
  );
}

function Strip({ strip }: { strip: BlockStrip }) {
  const sentence = stripSentence(strip);
  return (
    <div
      className="flex flex-col gap-2"
      data-strip={strip.waiting ? "waiting" : "filling"}
    >
      <div className="flex items-baseline justify-between gap-3">
        <span
          className={cn(
            "text-[0.8125rem]",
            strip.waiting ? "text-muted-foreground" : "text-foreground",
          )}
        >
          The frontier
        </span>
        <span className="text-xs text-muted-foreground tabular-nums">
          <Figures
            text={
              strip.waiting
                ? `${formatCount(strip.total)} blocks, waiting`
                : strip.toGo === 0
                  ? "all applied"
                  : `${formatCount(strip.toGo)} of ${formatCount(strip.total)} to go`
            }
          />
        </span>
      </div>
      {strip.waiting ? (
        // Nothing fetched yet: the count says it; a field of empty cells
        // would only be texture.
        <span className="sr-only">{sentence}</span>
      ) : (
        <>
          <div
            role="img"
            aria-label={sentence}
            className="grid grid-cols-[repeat(auto-fill,minmax(0.5rem,1fr))] gap-[3px]"
          >
            {strip.cells.map((cell, i) => (
              <span
                key={i}
                className={cn(
                  "aspect-square rounded-[2px] transition-colors duration-300",
                  CELL_CLASS[cell],
                )}
              />
            ))}
          </div>
          <StripLegend strip={strip} />
        </>
      )}
    </div>
  );
}

function StripLegend({ strip }: { strip: BlockStrip }) {
  return (
    <>
      <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <Legend
          className={CELL_CLASS.applied}
          label={`Applied ${formatCount(strip.applied)}`}
        />
        <Legend
          className={CELL_CLASS.fetched}
          label={`Fetched ${formatCount(Math.max(0, strip.fetched - strip.applied - strip.failed))}`}
        />
        <Legend
          className={CELL_CLASS.missing}
          label={`To fetch ${formatCount(Math.max(0, strip.total - strip.fetched))}`}
        />
        {strip.failed ? (
          <Legend
            className={CELL_CLASS.failed}
            label={`Failed ${formatCount(strip.failed)}`}
          />
        ) : null}
      </div>
    </>
  );
}

/** The ledgers, one bar each, from the node's sync progress. */
export function LedgerBars({ boot }: { boot: BootSnapshot }) {
  if (!boot.progress)
    return <Waiting text="Asking the node how far it has got." />;
  const rows = ledgerRows(boot.progress, boot.ledgerMemory);
  return (
    <ul className="flex flex-col gap-3" data-sync-graphic="ledgers">
      {rows.map(row => (
        <LedgerBar key={row.key} row={row} />
      ))}
    </ul>
  );
}

/** The frontier's blocks as cells, from the node's sync progress. */
export function FrontierStrip({ boot }: { boot: BootSnapshot }) {
  const strip = boot.progress ? blockStrip(boot.progress) : null;
  if (!strip) return null;
  return (
    <div data-sync-graphic="blocks">
      <Strip strip={strip} />
    </div>
  );
}

/** A line for a stage that has started but has no figures yet. */
function Waiting({ text }: { text: string }) {
  return (
    <p className="flex items-center gap-2 text-xs text-muted-foreground">
      <span className="h-1.5 w-8 rounded-full bg-status-devnet/60 motion-safe:animate-pulse" />
      {text}
    </p>
  );
}

/** Parts of a step in order: done, the current one, and those to come, as
 * a segmented bar with each part named. */
export function Parts({
  parts,
  current,
  label,
}: {
  parts: readonly string[];
  /** The current part's index; parts.length when all are done. */
  current: number;
  label: string;
}) {
  return (
    <div className="flex flex-col gap-2">
      <div
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={parts.length}
        aria-valuenow={Math.min(current, parts.length)}
        aria-valuetext={
          current < parts.length
            ? `${parts[current]}, part ${current + 1} of ${parts.length}`
            : "Done"
        }
        className="flex gap-1"
      >
        {parts.map((part, i) => (
          <span
            key={part}
            className={cn(
              "h-1.5 flex-1 rounded-full",
              i < current
                ? "bg-status-live"
                : i === current
                  ? "bg-status-devnet motion-safe:animate-pulse"
                  : "bg-muted",
            )}
          />
        ))}
      </div>
      <ol className="flex flex-col gap-1 text-xs">
        {parts.map((part, i) => (
          <li
            key={part}
            className={cn(
              "flex items-center gap-1.5",
              i === current ? "text-foreground" : "text-muted-foreground",
            )}
          >
            {i < current ? (
              <CircleCheck aria-hidden className="size-3 text-status-live" />
            ) : (
              <span
                aria-hidden
                className={cn(
                  "mx-0.5 size-2 rounded-full",
                  i === current
                    ? "bg-status-devnet"
                    : "border border-muted-foreground/50",
                )}
              />
            )}
            {/* No "2/3": beside a count of peers it reads as one. The
                marker and the bar say where it is. */}
            {part}
          </li>
        ))}
      </ol>
    </div>
  );
}
