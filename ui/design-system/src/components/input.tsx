import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/** A text field. Its border is --field, which keeps 3:1 against page and
 * card; aria-invalid turns it destructive. */
function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "flex h-10 w-full min-w-0 rounded-md border border-field bg-background/60 px-3 py-2 text-sm text-foreground transition-[color,border-color] duration-150 ease-standard outline-hidden placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
        "focus-visible:border-ring focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring",
        "aria-invalid:border-destructive",
        className,
      )}
      {...props}
    />
  );
}

export { Input };
