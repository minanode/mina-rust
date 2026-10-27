"use client";

import * as React from "react";
import { Check, Copy } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { copyName, copyText } from "@minanode/design-system/lib/clipboard";
import { cn } from "@minanode/design-system/lib/utils";

type CopyState = "idle" | "copied" | "failed";

/**
 * Copies `text`, on https or localhost through the Clipboard API and on a
 * plain-http address through a fallback (`lib/clipboard`). Its name says
 * what it copies, then that it did or couldn't; the state goes back after
 * 2 s.
 *
 * - `variant="icon"` (default): an icon button, `size="sm"` beside a value
 *   (Hash), `size="md"` with a border over a block (CodeBlock).
 * - `variant="text"`: a ghost button with words, `children` ("Copy JSON").
 */
function CopyButton({
  text,
  label,
  variant = "icon",
  size = "sm",
  state: forced,
  title,
  className,
  children,
}: {
  text: string;
  /** What it copies: "public key". */
  label: string;
  variant?: "icon" | "text";
  size?: "sm" | "md";
  /** Shows a state without a click (the design-system page). */
  state?: CopyState;
  /** The tooltip at rest, when it should say more than "Copy {label}". */
  title?: string;
  className?: string;
  /** The words of the text variant. */
  children?: React.ReactNode;
}) {
  const [own, setOwn] = React.useState<CopyState>("idle");
  const state = forced ?? own;
  const copy = () => {
    void copyText(text).then(ok => {
      setOwn(ok ? "copied" : "failed");
      setTimeout(() => setOwn("idle"), 2000);
    });
  };
  if (variant === "text") {
    return (
      <Button
        type="button"
        variant="ghost"
        size="sm"
        data-slot="copy-button"
        data-state={state}
        title={state === "idle" && title ? title : copyName(state, label)}
        onClick={copy}
        className={className}
      >
        <Copy
          aria-hidden
          className={cn(state === "failed" && "text-destructive")}
        />
        {state === "copied" ? "Copied" : children}
        <span className="sr-only">
          {state === "failed"
            ? `: couldn't copy, select the text instead`
            : ` of ${label}`}
        </span>
      </Button>
    );
  }
  const icon = size === "sm" ? "size-3.5" : "size-4";
  return (
    <button
      type="button"
      data-slot="copy-button"
      data-state={state}
      aria-label={copyName(state, label)}
      title={copyName(state, label)}
      onClick={copy}
      className={cn(
        "inline-flex shrink-0 items-center justify-center text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring",
        size === "sm"
          ? "size-6 rounded-sm"
          : "size-8 rounded-md border bg-background",
        className,
      )}
    >
      {state === "copied" ? (
        <Check aria-hidden className={cn(icon, "text-brand-text")} />
      ) : (
        <Copy
          aria-hidden
          className={cn(icon, state === "failed" && "text-destructive")}
        />
      )}
    </button>
  );
}

export { CopyButton, type CopyState };
