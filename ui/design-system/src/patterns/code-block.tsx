"use client";

import { useMoreToTheRight } from "@minanode/design-system/lib/more-to-the-right";
import { cn } from "@minanode/design-system/lib/utils";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";

/**
 * A block of code or a command to copy: mono, wraps long lines in a
 * focusable, named region, with a copy button named for what it copies.
 */
function CodeBlock({
  code,
  label,
  wrap = true,
  className,
}: {
  code: string;
  /** What the code is: "curl command". Names the region and the button. */
  label: string;
  /** False keeps each line whole and scrolls sideways: for a command split
   * into lines with `\`, where a wrapped part would read as a line of its
   * own. */
  wrap?: boolean;
  className?: string;
}) {
  // Kept whole, the lines scroll: the copy button takes a column of its
  // own rather than sitting over them, and the right edge fades while
  // more is hidden there, as tables do. On a phone, where a scroll hides
  // most of each line, they wrap instead, each line hanging so a wrapped
  // part reads as its continuation, not a line of its own.
  const more = useMoreToTheRight<HTMLPreElement>();
  const pre = (
    <pre
      tabIndex={0}
      role="region"
      aria-label={label}
      ref={wrap ? undefined : more.ref}
      data-more-right={!wrap && more.right ? true : undefined}
      className={cn(
        "overflow-x-auto py-4 pl-4 font-mono text-[0.8125rem] leading-relaxed text-foreground focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring",
        wrap
          ? "pr-14 whitespace-pre-wrap [overflow-wrap:anywhere]"
          : "min-w-0 pr-3 whitespace-pre max-sm:whitespace-pre-wrap max-sm:[overflow-wrap:anywhere] sm:data-more-right:[mask-image:linear-gradient(to_right,black_calc(100%-2.5rem),transparent)]",
      )}
    >
      {wrap ? (
        <code>{code}</code>
      ) : (
        <code>
          {/* A line's trailing "\" stays with its last word, never alone on
              a line; the copy keeps the plain text. */}
          {code.split("\n").map((line, i) => (
            <span
              key={i}
              className="block max-sm:pl-[4ch] max-sm:-indent-[4ch]"
            >
              {line.replace(/ \\$/, "\u00a0\\")}
            </span>
          ))}
        </code>
      )}
    </pre>
  );
  if (!wrap) {
    return (
      <div
        data-slot="code-block"
        className={cn(
          "grid grid-cols-[minmax(0,1fr)_auto] items-start rounded-xl border bg-card",
          className,
        )}
      >
        {pre}
        <CopyButton
          text={code}
          label={label}
          size="md"
          className="mt-2.5 mr-2.5"
        />
      </div>
    );
  }
  return (
    <div
      data-slot="code-block"
      className={cn("relative rounded-xl border bg-card", className)}
    >
      {pre}
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
