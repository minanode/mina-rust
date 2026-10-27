import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";
import { toneFill, type Tone } from "@minanode/design-system/patterns/signal";

/**
 * - done: it happened
 * - current: under way
 * - stopped: where it ended without this step
 * - skipped: not reached, it ended earlier
 * - pending: still to come
 */
type TimelineState = "done" | "current" | "stopped" | "skipped" | "pending";

type TimelineStep = {
  label: React.ReactNode;
  /** When, on the right: a time or an offset. */
  time?: React.ReactNode;
  detail?: React.ReactNode;
  state: TimelineState;
  /** The dot's colour for a done step; ok by default. */
  tone?: Tone;
};

const stateWords: Record<Exclude<TimelineState, "done">, string> = {
  current: "under way",
  stopped: "stopped here",
  skipped: "not reached",
  pending: "to come",
};

function Marker({ state, tone }: { state: TimelineState; tone: Tone }) {
  return (
    <span
      aria-hidden
      className="absolute top-1 left-0 flex size-3 items-center justify-center"
    >
      {state === "done" ? (
        <span className={cn("size-2.5 rounded-full", toneFill[tone])} />
      ) : state === "current" ? (
        <span className="relative flex size-2.5">
          <span className="absolute inset-0 rounded-full bg-status-devnet opacity-60 motion-safe:animate-ping" />
          <span className="relative size-2.5 rounded-full bg-status-devnet" />
        </span>
      ) : state === "stopped" ? (
        <span className="size-2.5 rounded-full border-2 border-status-pending bg-card" />
      ) : (
        <span
          className={cn(
            "size-2.5 rounded-full border bg-card",
            state === "skipped"
              ? "border-dashed border-muted-foreground/60"
              : "border-muted-foreground/60",
          )}
        />
      )}
    </span>
  );
}

/**
 * The steps of something that runs in order, as an ordered list: each with
 * a dot for how it went, its time on the right and a line of detail. Steps
 * not done say so in words as well as by their dot. Name it with `label`.
 */
function Timeline({
  steps,
  label,
  className,
}: {
  steps: TimelineStep[];
  label: string;
  className?: string;
}) {
  return (
    <ol
      aria-label={label}
      data-slot="timeline"
      className={cn("flex flex-col", className)}
    >
      {steps.map((step, i) => {
        const muted = step.state === "skipped" || step.state === "pending";
        return (
          <li
            key={i}
            data-state={step.state}
            className={cn(
              "relative pb-3 pl-6 last:pb-0",
              // The line to the next step.
              "before:absolute before:top-4.5 before:bottom-0.5 before:left-[0.34rem] before:w-px before:bg-border last:before:hidden",
            )}
          >
            <Marker state={step.state} tone={step.tone ?? "ok"} />
            <div className="flex flex-wrap items-baseline justify-between gap-x-3">
              <span
                className={cn(
                  "text-sm font-medium",
                  muted ? "text-muted-foreground" : "text-foreground",
                )}
              >
                {step.label}
                {step.state !== "done" ? (
                  <span className="ml-2 font-mono text-[0.6875rem] font-normal text-muted-foreground uppercase">
                    {stateWords[step.state]}
                  </span>
                ) : null}
              </span>
              {step.time ? (
                <span className="font-mono text-[0.8125rem] text-foreground tabular-nums">
                  {step.time}
                </span>
              ) : null}
            </div>
            {step.detail ? (
              <p className="pt-0.5 text-xs text-muted-foreground">
                {step.detail}
              </p>
            ) : null}
          </li>
        );
      })}
    </ol>
  );
}

export { Timeline, type TimelineState, type TimelineStep };
