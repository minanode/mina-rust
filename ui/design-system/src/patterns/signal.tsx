import * as React from "react";
import { cva } from "class-variance-authority";

import { cn } from "@minanode/design-system/lib/utils";

/**
 * The state of something running, such as a node's sync or a worker:
 * - ok: working as it should
 * - info: in progress, nothing wrong (connecting, catching up)
 * - warn: needs a look (an old tip, a failed job)
 * - error: not working (no answer, a crash)
 * - neutral: off, or not known yet
 *
 * StatusBadge is for evidence status on the site; Signal is for live state.
 * Always pair the colour with words.
 */
type Tone = "ok" | "info" | "warn" | "error" | "neutral";

const signalVariants = cva(
  "inline-flex w-fit shrink-0 items-center gap-2 rounded-full border px-2.5 py-1 font-mono text-[0.75rem] leading-none font-medium whitespace-nowrap",
  {
    variants: {
      tone: {
        ok: "border-status-live/35 bg-status-live/10 text-status-live",
        info: "border-status-devnet/35 bg-status-devnet/10 text-status-devnet",
        warn: "border-status-pending/35 bg-status-pending/10 text-status-pending",
        error: "border-destructive/40 bg-destructive/10 text-destructive",
        neutral: "border-border bg-muted/60 text-status-planned",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

/** A tone's solid fill, for dots and bars that carry the tone (pair them
 * with words). */
const toneFill: Record<Tone, string> = {
  ok: "bg-status-live",
  info: "bg-status-devnet",
  warn: "bg-status-pending",
  error: "bg-destructive",
  neutral: "bg-status-planned",
};

function SignalDot({
  tone = "neutral",
  pulse = false,
  className,
}: {
  tone?: Tone;
  pulse?: boolean;
  className?: string;
}) {
  return (
    <span
      aria-hidden
      data-slot="signal-dot"
      className={cn("relative inline-flex size-2 shrink-0", className)}
    >
      {pulse ? (
        <span
          className={cn(
            "absolute inset-0 rounded-full opacity-60 motion-safe:animate-ping",
            toneFill[tone],
          )}
        />
      ) : null}
      <span
        className={cn(
          "relative inline-flex size-2 rounded-full",
          toneFill[tone],
        )}
      />
    </span>
  );
}

function Signal({
  tone = "neutral",
  pulse,
  children,
  className,
  ...props
}: React.ComponentProps<"span"> & { tone?: Tone; pulse?: boolean }) {
  return (
    <span
      data-slot="signal"
      data-tone={tone}
      className={cn(signalVariants({ tone }), className)}
      {...props}
    >
      <SignalDot tone={tone} pulse={pulse} />
      {children}
    </span>
  );
}

export { Signal, SignalDot, signalVariants, toneFill, type Tone };
