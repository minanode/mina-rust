import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { borderlessInSurface } from "@minanode/design-system/lib/surface";
import { cn } from "@minanode/design-system/lib/utils";

const alertVariants = cva(
  "relative grid w-full grid-cols-[0_1fr] items-start gap-y-1 rounded-lg border px-4 py-3.5 text-sm has-[>svg]:grid-cols-[calc(var(--spacing)*4)_1fr] has-[>svg]:gap-x-3 [&>svg]:size-4 [&>svg]:translate-y-0.5 [&>svg]:text-current",
  {
    variants: {
      variant: {
        default: "bg-card text-card-foreground",
        // In the apps, the description's muted text a step darker on the
        // tint, for AA in light.
        brand:
          "border-brand-border bg-brand-wash text-foreground in-data-app:[&_[data-slot=alert-description]]:text-muted-on-tint [&>svg]:text-brand-text",
        // Inside a card, a stronger tint: the 8% one reads grey there.
        caution:
          "border-status-pending/40 bg-caution-wash text-foreground in-data-[slot=card]:bg-caution-wash-strong in-data-app:[&_[data-slot=alert-description]]:text-muted-on-tint [&>svg]:text-status-pending",
        destructive:
          "border-destructive/40 bg-destructive/8 text-foreground in-data-[slot=card]:bg-destructive/12 in-data-app:[&_[data-slot=alert-description]]:text-muted-on-tint [&>svg]:text-destructive",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  },
);

function Alert({
  className,
  variant,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof alertVariants>) {
  return (
    <div
      data-slot="alert"
      role="note"
      className={cn(alertVariants({ variant }), borderlessInSurface, className)}
      {...props}
    />
  );
}

function AlertTitle({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-title"
      className={cn(
        // The apps set it at normal tracking; the site keeps its own.
        "col-start-2 min-h-4 font-medium tracking-tight in-data-app:tracking-normal",
        className,
      )}
      {...props}
    />
  );
}

function AlertDescription({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-description"
      className={cn(
        "col-start-2 grid justify-items-start gap-1 text-sm leading-relaxed text-pretty text-muted-foreground [&_p]:leading-relaxed",
        className,
      )}
      {...props}
    />
  );
}

export { Alert, AlertTitle, AlertDescription, alertVariants };
