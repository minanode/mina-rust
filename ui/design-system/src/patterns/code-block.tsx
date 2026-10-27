"use client";

import { cn } from "@minanode/design-system/lib/utils";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";

/**
 * A block of code or a command to copy: mono, wraps long lines in a
 * focusable, named region, with a copy button named for what it copies.
 */
function CodeBlock({
  code,
  label,
  className,
}: {
  code: string;
  /** What the code is: "curl command". Names the region and the button. */
  label: string;
  className?: string;
}) {
  return (
    <div
      data-slot="code-block"
      className={cn("relative rounded-xl border bg-card", className)}
    >
      <pre
        tabIndex={0}
        role="region"
        aria-label={label}
        className="overflow-x-auto whitespace-pre-wrap [overflow-wrap:anywhere] py-4 pr-14 pl-4 font-mono text-[0.8125rem] leading-relaxed text-foreground focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring"
      >
        <code>{code}</code>
      </pre>
      <CopyButton
        text={code}
        label={label}
        size="md"
        className="absolute top-2.5 right-2.5 bg-card/85 backdrop-blur-xs"
      />
    </div>
  );
}

export { CodeBlock };
