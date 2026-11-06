import * as React from "react";

import { flatInSurface } from "@minanode/design-system/lib/surface";
import { cn } from "@minanode/design-system/lib/utils";

/**
 * What a panel says when it has nothing to show: no data yet, a feature
 * that's off, or a source that doesn't answer. An icon, a title, a sentence
 * on what to do, and an optional action, beside the icon. On its own it is
 * a bordered panel; inside a Card or a KeyValueList it is flat, with no box
 * of its own (`lib/surface`).
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
  // With an icon: the icon beside the title. From 640 px the text and the
  // action keep to the title's column; on a phone they take the full width.
  const below = icon ? "col-span-2 sm:col-span-1 sm:col-start-2" : undefined;
  return (
    <div
      data-slot="empty-state"
      className={cn(
        "grid items-start gap-x-4 gap-y-1 rounded-xl border p-4",
        icon ? "grid-cols-[auto_minmax(0,1fr)]" : "grid-cols-1",
        tone === "default" && "bg-card",
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
            "inline-flex size-9 items-center justify-center rounded-lg border bg-background sm:row-span-2 [&_svg]:size-[1.125rem]",
            tone === "default" && "text-brand-text",
            tone === "caution" && "text-status-pending",
            tone === "destructive" && "text-destructive",
          )}
        >
          {icon}
        </span>
      ) : null}
      <p
        className={cn(
          "min-w-0 font-semibold text-balance text-foreground",
          icon && "self-center sm:self-start sm:pt-1.5",
        )}
      >
        {title}
      </p>
      {description ? (
        <div
          className={cn(
            "max-w-prose text-sm leading-relaxed text-pretty text-muted-foreground",
            icon && "pt-2 sm:pt-0",
            below,
          )}
        >
          {description}
        </div>
      ) : null}
      {action ? (
        <div className={cn("flex flex-wrap gap-2 pt-3", below)}>{action}</div>
      ) : null}
    </div>
  );
}

export { EmptyState };
