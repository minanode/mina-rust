import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/**
 * A headline number with its label and the caveat that has to travel with
 * it. Keep the caveat: on minanode.org every number maps to a source line in
 * CLAIMS.md.
 */
function MetricCard({
  value,
  label,
  detail,
  footer,
  size = "default",
  className,
  ...props
}: Omit<React.ComponentProps<"div">, "children"> & {
  value: React.ReactNode;
  label: React.ReactNode;
  detail?: React.ReactNode;
  footer?: React.ReactNode;
  size?: "default" | "sm";
}) {
  return (
    <div
      data-slot="metric-card"
      className={cn(
        "@container flex flex-col gap-2 rounded-xl border bg-card text-card-foreground shadow-card",
        size === "default" ? "p-6" : "p-4",
        className,
      )}
      {...props}
    >
      <p
        className={cn(
          "font-mono text-brand-text tabular-nums",
          size === "default"
            ? "text-metric whitespace-nowrap"
            : "text-xl font-semibold",
        )}
        // Scale with the card so long values such as "15,897 / 15,897" stay
        // on one line; text-metric is the upper bound. Inline so that
        // tailwind-merge cannot drop text-metric's line height and weight.
        style={
          size === "default"
            ? { fontSize: "min(var(--text-metric), 10cqi)" }
            : undefined
        }
      >
        {value}
      </p>
      <p
        className={cn(
          "font-semibold text-foreground",
          size === "default" ? "text-base" : "text-sm",
        )}
      >
        {label}
      </p>
      {detail ? (
        <p className="text-sm leading-relaxed text-muted-foreground">
          {detail}
        </p>
      ) : null}
      {footer ? <div className="mt-auto pt-2 text-sm">{footer}</div> : null}
    </div>
  );
}

export { MetricCard };
