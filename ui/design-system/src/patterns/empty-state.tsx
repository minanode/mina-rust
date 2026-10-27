import * as React from "react";

import { flatInSurface } from "@minanode/design-system/lib/surface";
import { cn } from "@minanode/design-system/lib/utils";

/**
 * What a panel says when it has nothing to show: no data yet, a feature
 * that's off, or a source that doesn't answer. An icon, a title, a sentence
 * on what to do, and an optional action. On its own it is a dashed box;
 * inside a Card or a KeyValueList it is flat, with no box of its own
 * (`lib/surface`).
 */
function EmptyState({
  icon,
  title,
  description,
  action,
  tone = "default",
  className,
  ...props
}: Omit<React.ComponentProps<"div">, "title"> & {
  icon?: React.ReactNode;
  title: React.ReactNode;
  description?: React.ReactNode;
  action?: React.ReactNode;
  tone?: "default" | "caution" | "destructive";
}) {
  return (
    <div
      data-slot="empty-state"
      className={cn(
        "flex flex-col items-start gap-3 rounded-xl border border-dashed px-6 py-8",
        tone === "default" && "bg-card/50",
        tone === "caution" && "border-status-pending/40 bg-status-pending/5",
        tone === "destructive" && "border-destructive/40 bg-destructive/5",
        flatInSurface,
        className,
      )}
      {...props}
    >
      {icon ? (
        <span
          aria-hidden
          className={cn(
            "inline-flex size-10 items-center justify-center rounded-lg border bg-background [&_svg]:size-5",
            tone === "default" && "text-brand-text",
            tone === "caution" && "text-status-pending",
            tone === "destructive" && "text-destructive",
          )}
        >
          {icon}
        </span>
      ) : null}
      <div className="flex max-w-prose flex-col gap-1.5">
        <p className="font-semibold text-foreground">{title}</p>
        {description ? (
          <div className="text-sm leading-relaxed text-muted-foreground">
            {description}
          </div>
        ) : null}
      </div>
      {action ? <div className="pt-1">{action}</div> : null}
    </div>
  );
}

export { EmptyState };
