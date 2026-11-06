"use client";

import * as React from "react";
import { Tabs as TabsPrimitive } from "radix-ui";

import { useMoreToTheRight } from "@minanode/design-system/lib/more-to-the-right";
import { cn } from "@minanode/design-system/lib/utils";

function Tabs({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn("flex flex-col gap-3", className)}
      {...props}
    />
  );
}

function TabsList({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.List>) {
  // One line at every width: on a phone it scrolls sideways, fading at the
  // right while tabs are hidden there. The rule under it is a shadow, so it
  // stays put as the tabs scroll over it.
  const more = useMoreToTheRight<HTMLDivElement>();
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      ref={more.ref}
      data-more-right={more.right || undefined}
      className={cn(
        "flex w-full max-w-full items-end gap-x-2 overflow-x-auto text-muted-foreground shadow-[inset_0_-1px_0_var(--color-border)] [scrollbar-width:none] data-more-right:[mask-image:linear-gradient(to_right,black_calc(100%-2.5rem),transparent)] [&::-webkit-scrollbar]:hidden",
        className,
      )}
      {...props}
    />
  );
}

function TabsTrigger({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        "inline-flex h-10 shrink-0 items-center justify-center gap-1.5 rounded-t-md border-b-2 border-transparent px-2 text-sm font-medium whitespace-nowrap text-muted-foreground transition-[color,border-color] duration-150 ease-standard hover:text-foreground focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 data-[state=active]:border-brand data-[state=active]:text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className,
      )}
      {...props}
    />
  );
}

function TabsContent({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      className={cn(
        "flex-1 rounded-md outline-hidden focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-4 focus-visible:outline-ring",
        className,
      )}
      {...props}
    />
  );
}

export { Tabs, TabsList, TabsTrigger, TabsContent };
