import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

type LogoMarkProps = Omit<React.ComponentProps<"svg">, "children"> & {
  /** Kept for compatibility with the earlier tile mark; it has no effect. */
  shape?: "rounded" | "square";
  /** Accessible name. Leave empty when the mark sits next to visible text. */
  title?: string;
};

/**
 * Twin arches: two equal arches forming an "m", in brand teal (#12A594),
 * on a 48 x 48 grid. The stroke uses currentColor, so a class can recolour
 * it.
 */
function LogoMark({
  shape: _shape,
  title,
  className,
  ...props
}: LogoMarkProps) {
  const titleId = React.useId();
  return (
    <svg
      viewBox="0 0 48 48"
      fill="none"
      data-slot="logo-mark"
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      aria-labelledby={title ? titleId : undefined}
      className={cn("size-8 shrink-0 text-teal-500", className)}
      {...props}
    >
      {title ? <title id={titleId}>{title}</title> : null}
      <path
        d="M6 36V22C6 9 24 9 24 22V36M24 22C24 9 42 9 42 22V36"
        stroke="currentColor"
        strokeWidth="5.5"
        strokeLinecap="square"
      />
    </svg>
  );
}

/** Mark plus the wordmark: "mina" semibold, "node" lighter and muted. */
function Logo({
  className,
  markClassName,
  shape,
  ...props
}: React.ComponentProps<"span"> & {
  markClassName?: string;
  shape?: LogoMarkProps["shape"];
}) {
  return (
    <span
      data-slot="logo"
      className={cn(
        "inline-flex items-center gap-[11px] text-[22px] leading-none tracking-[-0.045em] text-foreground",
        className,
      )}
      {...props}
    >
      <LogoMark shape={shape} className={markClassName} />
      <span>
        <span className="font-semibold">mina</span>
        <span className="font-normal text-muted-foreground">node</span>
      </span>
    </span>
  );
}

export { Logo, LogoMark };
