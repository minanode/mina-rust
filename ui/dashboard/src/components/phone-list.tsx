import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

export type PhoneRow = {
  key: React.Key;
  /** What the row is (a block, a slot): on the left. */
  lead: React.ReactNode;
  /** What decides it (a status, a fee): on the right. */
  end?: React.ReactNode;
  /** The rest, small and muted, figures in the text colour. */
  meta: React.ReactNode[];
  /** A second line of it, so rows keep one shape when the first would
   * wrap on some and not others. */
  more?: React.ReactNode[];
};

/**
 * A table's rows on a phone, as Recent jobs, Blocks and Peers show theirs:
 * one item a row, so no column scrolls out of view. The table beside it
 * takes `PHONE_HIDDEN` on its container.
 */
export function PhoneList({
  label,
  rows,
  className,
}: {
  label: string;
  rows: PhoneRow[];
  className?: string;
}) {
  return (
    <ul
      aria-label={label}
      className={cn(
        "flex flex-col divide-y rounded-xl border bg-card sm:hidden",
        className,
      )}
    >
      {rows.map(r => (
        <li key={r.key} className="flex flex-col gap-1 px-4 py-3">
          <div className="flex items-center justify-between gap-3 text-sm">
            <span className="min-w-0">{r.lead}</span>
            {r.end ? <span className="shrink-0">{r.end}</span> : null}
          </div>
          {[r.meta, r.more].map((line, n) =>
            line ? (
              <p
                key={n}
                className="flex flex-wrap gap-x-3 text-xs text-muted-foreground tabular-nums"
              >
                {line.filter(Boolean).map((m, i) => (
                  <span key={i}>{m}</span>
                ))}
              </p>
            ) : null,
          )}
        </li>
      ))}
    </ul>
  );
}

/** The container class for a table a PhoneList stands in for on a phone. */
export const PHONE_HIDDEN = "hidden sm:block";

/** A count and its noun, the figure in mono and the text colour: "104
 * jobs". */
export function CountOf({
  n,
  one,
  many = `${one}s`,
}: {
  n: number;
  one: string;
  many?: string;
}) {
  return (
    <>
      <span className="font-mono text-foreground">
        {n.toLocaleString("en-US")}
      </span>{" "}
      {n === 1 ? one : many}
    </>
  );
}
