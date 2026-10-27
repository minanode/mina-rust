import * as React from "react";

import { Button } from "@minanode/design-system/components/button";

/**
 * A small set of exclusive choices that change what a page shows (which
 * peers, which answer): toggle buttons with aria-pressed in a group. For
 * panels of content, use Tabs.
 */
function SegmentedControl<T extends string>({
  label,
  options,
  value,
  onValueChange,
}: {
  /** The group's accessible name: "Which peers". */
  label: string;
  options: readonly { value: T; label: React.ReactNode }[];
  value: T;
  onValueChange: (value: T) => void;
}) {
  return (
    <div
      data-slot="segmented-control"
      role="group"
      aria-label={label}
      className="inline-flex w-fit items-center gap-1 rounded-lg border bg-muted/50 p-1"
    >
      {options.map(option => (
        <Button
          key={option.value}
          type="button"
          size="sm"
          variant="ghost"
          aria-pressed={value === option.value}
          onClick={() => onValueChange(option.value)}
          className="font-mono text-muted-foreground aria-pressed:border aria-pressed:border-brand-border aria-pressed:bg-brand-subtle aria-pressed:text-brand-text"
        >
          {option.label}
        </Button>
      ))}
    </div>
  );
}

export { SegmentedControl };
