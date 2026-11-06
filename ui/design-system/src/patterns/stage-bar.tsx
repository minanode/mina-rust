import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";
import { toneFill, type Tone } from "@minanode/design-system/patterns/signal";

type StageBarSpan = { start: number; end: number; tone?: Tone };
type StageBarMark = { at: number; tone?: Tone };
type StageBarTick = { at: number; label: React.ReactNode };

/**
 * Where things fell in a window of time, on one line: a track over
 * `domain`, the `window` that matters lit on it (a slot, a deadline),
 * `spans` for stages that took time, and `marks` for moments. Numbers are in
 * any one unit, such as ms from the window's start. An image to assistive
 * tech, named by `label`: say in it what the bar shows, and give the figures
 * in words nearby too.
 */
function StageBar({
  domain,
  window,
  spans = [],
  marks = [],
  ticks = [],
  ticksClassName,
  label,
  className,
}: {
  domain: [number, number];
  window?: { start: number; end: number };
  spans?: StageBarSpan[];
  marks?: StageBarMark[];
  ticks?: StageBarTick[];
  /** For the tick row: `md:hidden` where a list draws one axis above its
   * rows (StageBarAxis). */
  ticksClassName?: string;
  label: string;
  className?: string;
}) {
  const [from, to] = domain;
  const width = to - from || 1;
  const pct = (v: number) =>
    Math.min(100, Math.max(0, ((v - from) / width) * 100));
  const bar = (start: number, end: number) => ({
    left: `${pct(start)}%`,
    width: `${Math.max(pct(end) - pct(start), 0.6)}%`,
  });
  return (
    <div
      role="img"
      aria-label={label}
      data-slot="stage-bar"
      className={cn("flex min-w-0 flex-col gap-1", className)}
    >
      <div className="relative h-3">
        {/* One pill: the lead-in a quiet step of the track's colour, the
            window a step stronger, the spans square fills inside it, all
            clipped to the track's ends. Marks sit over it, unclipped. */}
        <div className="absolute inset-x-0 top-1/2 h-1.5 -translate-y-1/2 overflow-hidden rounded-full bg-muted-foreground/12">
          {window ? (
            <span
              data-part="window"
              className="absolute inset-y-0 bg-muted-foreground/25"
              style={bar(window.start, window.end)}
            />
          ) : null}
          {spans.map((s, i) => (
            <span
              key={i}
              data-part="span"
              className={cn("absolute inset-y-0", toneFill[s.tone ?? "info"])}
              style={bar(s.start, s.end)}
            />
          ))}
        </div>
        {marks.map((m, i) => (
          <span
            key={i}
            data-part="mark"
            className={cn(
              "absolute top-1/2 size-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full ring-2 ring-card",
              toneFill[m.tone ?? "ok"],
            )}
            style={{ left: `${pct(m.at)}%` }}
          />
        ))}
      </div>
      {ticks.length ? (
        <StageBarAxis
          domain={domain}
          ticks={ticks}
          className={ticksClassName}
        />
      ) : null}
    </div>
  );
}

/** A StageBar's tick labels on their own: one axis above a list of bars on
 * the same domain, so each row needn't repeat it. */
function StageBarAxis({
  domain,
  ticks,
  className,
}: {
  domain: [number, number];
  ticks: StageBarTick[];
  className?: string;
}) {
  const [from, to] = domain;
  const width = to - from || 1;
  const pct = (v: number) =>
    Math.min(100, Math.max(0, ((v - from) / width) * 100));
  return (
    <div
      aria-hidden
      data-slot="stage-bar-axis"
      className={cn(
        "relative h-4 font-mono text-[0.6875rem] leading-4 text-muted-foreground",
        className,
      )}
    >
      {ticks.map((t, i) => {
        const at = pct(t.at);
        return (
          <span
            key={i}
            className={cn(
              "absolute whitespace-nowrap",
              at < 8 ? "" : at > 92 ? "-translate-x-full" : "-translate-x-1/2",
            )}
            style={{ left: `${at}%` }}
          >
            {t.label}
          </span>
        );
      })}
    </div>
  );
}

export {
  StageBar,
  StageBarAxis,
  type StageBarMark,
  type StageBarSpan,
  type StageBarTick,
};
