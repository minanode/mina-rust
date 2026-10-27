import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/** A multi-line text field, styled as Input: --field border (3:1 against
 * page and card), aria-invalid turns it destructive. */
function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "flex min-h-20 w-full min-w-0 rounded-md border border-field bg-background/60 px-3 py-2 text-sm text-foreground transition-[color,border-color] duration-150 ease-standard outline-hidden placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
        "focus-visible:border-ring focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring",
        "aria-invalid:border-destructive",
        className,
      )}
      {...props}
    />
  );
}

export { Textarea };
