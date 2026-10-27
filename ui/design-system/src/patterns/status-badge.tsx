import * as React from "react";
import { cva } from "class-variance-authority";

import { cn } from "@minanode/design-system/lib/utils";

/**
 * Evidence status, matching the scorecard in the private qualification notes:
 * - live: working, on main
 * - devnet: shown on public devnet
 * - pending: in progress, on an isolated network, or in preparation
 * - planned: not started or not public yet
 */
type Status = "live" | "devnet" | "pending" | "planned";

const statusVariants = cva(
  "inline-flex w-fit shrink-0 items-center gap-2 rounded-full border px-2.5 py-1 font-mono text-[0.75rem] leading-none font-medium whitespace-nowrap",
  {
    variants: {
      status: {
        live: "border-status-live/35 bg-status-live/10 text-status-live",
        devnet: "border-status-devnet/35 bg-status-devnet/10 text-status-devnet",
        pending:
          "border-status-pending/35 bg-status-pending/10 text-status-pending",
        planned: "border-border bg-muted/60 text-status-planned",
      },
    },
    defaultVariants: { status: "live" },
  },
);

const defaultLabels: Record<Status, string> = {
  live: "Working",
  devnet: "Shown on public devnet",
  pending: "In progress",
  planned: "Planned",
};

function StatusDot({
  status = "live",
  pulse = false,
  className,
}: {
  status?: Status;
  pulse?: boolean;
  className?: string;
}) {
  const color = {
    live: "bg-status-live",
    devnet: "bg-status-devnet",
    pending: "bg-status-pending",
    planned: "bg-status-planned",
  }[status];
  return (
    <span
      aria-hidden
      data-slot="status-dot"
      className={cn("relative inline-flex size-2 shrink-0", className)}
    >
      {pulse ? (
        <span
          className={cn(
            "absolute inset-0 rounded-full opacity-60 motion-safe:animate-ping",
            color,
          )}
        />
      ) : null}
      <span className={cn("relative inline-flex size-2 rounded-full", color)} />
    </span>
  );
}

function StatusBadge({
  status = "live",
  pulse,
  children,
  className,
  ...props
}: React.ComponentProps<"span"> & { status?: Status; pulse?: boolean }) {
  return (
    <span
      data-slot="status-badge"
      data-status={status}
      className={cn(statusVariants({ status }), className)}
      {...props}
    >
      <StatusDot status={status} pulse={pulse} />
      {children ?? defaultLabels[status]}
    </span>
  );
}

export { StatusBadge, StatusDot, statusVariants, type Status };
