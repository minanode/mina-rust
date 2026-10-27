import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

type Row = {
  label: React.ReactNode;
  value: React.ReactNode;
  /** Brand colour for values worth drawing the eye to. */
  tone?: "default" | "brand";
};

/**
 * A monospace receipt: a titled panel of label and value rows, rendered as a
 * description list.
 */
function KeyValueList({
  align = "right",
  title,
  aside,
  rows,
  footer,
  className,
  ...props
}: Omit<React.ComponentProps<"div">, "title" | "children"> & {
  align?: "left" | "right";
  title: React.ReactNode;
  aside?: React.ReactNode;
  rows: Row[];
  footer?: React.ReactNode;
}) {
  return (
    <div
      data-slot="key-value-list"
      className={cn(
        "flex flex-col rounded-xl border bg-card in-[.bg-card]:bg-background/60",
        align === "left" ? "font-sans text-sm" : "font-mono text-[0.8125rem]",
        className,
      )}
      {...props}
    >
      <div className="flex flex-wrap items-center justify-between gap-2 border-b px-4 py-3">
        <p className="font-medium text-foreground">{title}</p>
        {aside}
      </div>
      <dl className="flex flex-col gap-2.5 px-4 py-4">
        {rows.map((row, i) => (
          <div
            key={i}
            className="grid grid-cols-1 gap-0.5 sm:grid-cols-[minmax(9rem,auto)_1fr] sm:gap-4"
          >
            <dt className="text-muted-foreground">{row.label}</dt>
            <dd
              className={cn(
                "break-words",
                align === "right" && "sm:text-right",
                row.tone === "brand" ? "text-brand-text" : "text-foreground",
              )}
            >
              {row.value}
            </dd>
          </div>
        ))}
      </dl>
      {footer ? (
        <div className="border-t px-4 py-3 font-sans text-sm text-muted-foreground">
          {footer}
        </div>
      ) : null}
    </div>
  );
}

export { KeyValueList, type Row as KeyValueRow };
