import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/**
 * A compact number with its label and caveat, without a card: for rows of
 * figures inside a panel.
 *
 * - `variant="default"`: the value first in brand teal, scaling with the
 *   stat's width and never wrapping, then the label and a short caveat
 *   (the site's figures).
 * - `variant="labelled"`: a mono label first, then the value in the text
 *   colour and a sentence of detail (a dashboard's live figures).
 *
 * `inList` (labelled variant) renders the parts as `dt` and `dd`, for a
 * stat inside a `<dl>`. `size="sm"` (labelled variant) sets a smaller value,
 * for many figures in a row (StatRow).
 */
function Stat({
  value,
  label,
  detail,
  variant = "default",
  inList = false,
  size = "default",
  className,
  ...props
}: Omit<React.ComponentProps<"div">, "children"> & {
  value: React.ReactNode;
  label: React.ReactNode;
  detail?: React.ReactNode;
  variant?: "default" | "labelled";
  inList?: boolean;
  size?: "default" | "sm";
}) {
  const Term = inList ? "dt" : "p";
  const Desc = inList ? "dd" : "p";
  if (variant === "labelled") {
    return (
      <div
        data-slot="stat"
        className={cn("flex min-w-0 flex-col gap-1", className)}
        {...props}
      >
        <Term className="font-mono text-label text-muted-foreground uppercase">
          {label}
        </Term>
        <Desc
          className={cn(
            "font-mono font-semibold text-foreground tabular-nums",
            size === "sm" ? "text-lg" : "text-xl sm:text-2xl",
          )}
        >
          {value}
        </Desc>
        {detail ? (
          <Desc className="text-sm text-muted-foreground">{detail}</Desc>
        ) : null}
      </div>
    );
  }
  return (
    <div
      data-slot="stat"
      className={cn("@container flex min-w-0 flex-col gap-1", className)}
      {...props}
    >
      <p
        className="font-mono font-semibold whitespace-nowrap text-brand-text tabular-nums"
        // Upper bound 1.375rem; shrinks so "16.2–17.3 s" fits a narrow column.
        style={{ fontSize: "min(1.375rem, 14cqi)", lineHeight: 1.2 }}
      >
        {value}
      </p>
      <p className="text-sm font-medium text-foreground">{label}</p>
      {detail ? (
        <p className="text-xs leading-relaxed text-muted-foreground">
          {detail}
        </p>
      ) : null}
    </div>
  );
}

export { Stat };
