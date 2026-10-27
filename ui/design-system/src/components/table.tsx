import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

function Table({
  className,
  containerProps,
  ...props
}: React.ComponentProps<"table"> & {
  /** Props for the scroll container. When the table can overflow, give it a
   * `tabIndex={0}`, `role="region"` and an accessible name so keyboard users
   * can scroll it. */
  containerProps?: React.ComponentProps<"div">;
}) {
  return (
    <div
      data-slot="table-container"
      {...containerProps}
      className={cn(
        "relative w-full overflow-x-auto rounded-xl border bg-card",
        containerProps?.className,
      )}
    >
      <table
        data-slot="table"
        className={cn("w-full caption-bottom text-sm", className)}
        {...props}
      />
    </div>
  );
}

function TableHeader({ className, ...props }: React.ComponentProps<"thead">) {
  return (
    <thead
      data-slot="table-header"
      className={cn("bg-muted/40 [&_tr]:border-b", className)}
      {...props}
    />
  );
}

function TableBody({ className, ...props }: React.ComponentProps<"tbody">) {
  return (
    <tbody
      data-slot="table-body"
      className={cn("[&_tr:last-child]:border-0", className)}
      {...props}
    />
  );
}

function TableFooter({ className, ...props }: React.ComponentProps<"tfoot">) {
  return (
    <tfoot
      data-slot="table-footer"
      className={cn(
        "border-t bg-muted/50 font-medium [&>tr]:last:border-b-0",
        className,
      )}
      {...props}
    />
  );
}

function TableRow({ className, ...props }: React.ComponentProps<"tr">) {
  return (
    <tr
      data-slot="table-row"
      className={cn(
        "border-b transition-colors duration-150 hover:bg-muted/40 data-[state=selected]:bg-brand-subtle",
        className,
      )}
      {...props}
    />
  );
}

function TableHead({ className, ...props }: React.ComponentProps<"th">) {
  return (
    <th
      data-slot="table-head"
      className={cn(
        "h-11 px-4 text-left align-middle font-mono text-label whitespace-nowrap text-muted-foreground uppercase [&:has([role=checkbox])]:pr-0",
        className,
      )}
      {...props}
    />
  );
}

function TableCell({ className, ...props }: React.ComponentProps<"td">) {
  return (
    <td
      data-slot="table-cell"
      className={cn(
        "px-4 py-3.5 align-top [&:has([role=checkbox])]:pr-0",
        className,
      )}
      {...props}
    />
  );
}

function TableCaption({
  className,
  ...props
}: React.ComponentProps<"caption">) {
  return (
    <caption
      data-slot="table-caption"
      className={cn(
        "border-t px-4 py-3 text-left text-sm text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

/** Props for a table's scroll container: a focusable, named region, so
 * keyboard users can scroll it. Its name ends in "table", so it differs from
 * a section around it of the same name. */
function tableRegion(label: string) {
  return {
    tabIndex: 0,
    role: "region",
    "aria-label": `${label} table`,
  } as const;
}

export {
  tableRegion,
  Table,
  TableHeader,
  TableBody,
  TableFooter,
  TableHead,
  TableRow,
  TableCell,
  TableCaption,
};
