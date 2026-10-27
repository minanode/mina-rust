"use client";

import * as React from "react";
import { ChevronRight } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { cn } from "@minanode/design-system/lib/utils";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";

/**
 * A JSON value as a collapsible tree: objects and arrays fold, the first
 * levels start open, long arrays show a page at a time, and the whole value
 * can be copied as pretty-printed JSON. It knows nothing about where the
 * value came from.
 *
 * Folding is kept by path while `value` changes (a polled answer), so the
 * tree doesn't jump as the data refreshes.
 */

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** The JSON a copy gives: two-space indents, as `JSON.stringify` writes. */
export function jsonText(value: unknown): string {
  return JSON.stringify(value, null, 2) ?? "undefined";
}

/** How a folded object or array reads: `{4 keys}`, `[64 items]`. */
export function foldSummary(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.length} ${value.length === 1 ? "item" : "items"}]`;
  }
  const keys = Object.keys(value as object).length;
  return `{${keys} ${keys === 1 ? "key" : "keys"}}`;
}

function isBranch(value: unknown): value is object {
  return value !== null && typeof value === "object";
}

type Fold = {
  /** Open by default at depths below this. */
  openDepth: number;
  /** Set by expand all and collapse all; null when neither was used. */
  all: boolean | null;
  overrides: Map<string, boolean>;
  toggle: (path: string, open: boolean) => void;
  shown: Map<string, number>;
  showMore: (path: string, count: number) => void;
  pageSize: number;
};

const FoldContext = React.createContext<Fold | null>(null);

function Primitive({ value }: { value: unknown }) {
  if (value === null) {
    return <span className="text-muted-foreground italic">null</span>;
  }
  if (typeof value === "string") {
    return (
      <span className="break-all text-foreground">&quot;{value}&quot;</span>
    );
  }
  if (typeof value === "number") {
    return (
      <span className="text-brand-text tabular-nums">{String(value)}</span>
    );
  }
  if (typeof value === "boolean") {
    return <span className="text-status-devnet">{String(value)}</span>;
  }
  return <span className="text-muted-foreground">{String(value)}</span>;
}

function Key({ name }: { name: string | number | null }) {
  if (name === null) return null;
  return (
    <span className="text-muted-foreground">
      {typeof name === "number" ? name : `"${name}"`}:{" "}
    </span>
  );
}

function Node({
  name,
  value,
  path,
  depth,
}: {
  name: string | number | null;
  value: unknown;
  path: string;
  depth: number;
}) {
  const fold = React.useContext(FoldContext)!;
  const childrenId = React.useId();
  if (!isBranch(value)) {
    return (
      <li className="pl-5">
        <Key name={name} />
        <Primitive value={value} />
      </li>
    );
  }
  const entries: [string | number, unknown][] = Array.isArray(value)
    ? value.map((item, i) => [i, item])
    : Object.entries(value);
  const open =
    entries.length > 0 &&
    (fold.overrides.get(path) ??
      (fold.all !== null ? fold.all || depth === 0 : depth < fold.openDepth));
  const shown = Math.min(entries.length, fold.shown.get(path) ?? fold.pageSize);
  const kind = Array.isArray(value) ? "array" : "object";
  // The visible text names the button; this adds what it is, after it.
  const more = ` ${name === null ? "root " : ""}${kind}, ${foldSummary(value).slice(1, -1)}`;
  return (
    <li>
      <button
        type="button"
        aria-expanded={entries.length > 0 ? open : undefined}
        aria-controls={open ? childrenId : undefined}
        disabled={entries.length === 0}
        onClick={() => fold.toggle(path, !open)}
        className="group inline-flex min-h-6 py-0.5 max-w-full items-start gap-1 rounded-sm text-left hover:bg-accent focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none"
      >
        <ChevronRight
          aria-hidden
          className={cn(
            "mt-0.5 size-4 shrink-0 text-muted-foreground transition-transform duration-150",
            open && "rotate-90",
            entries.length === 0 && "invisible",
          )}
        />
        <span>
          <Key name={name} />
          <span className="text-muted-foreground">
            {entries.length === 0
              ? Array.isArray(value)
                ? "[]"
                : "{}"
              : open
                ? Array.isArray(value)
                  ? "["
                  : "{"
                : foldSummary(value)}
          </span>
          <span className="sr-only">{more}</span>
        </span>
      </button>
      {open ? (
        <>
          <ul id={childrenId} className="ml-2 border-l border-border pl-2">
            {entries.slice(0, shown).map(([key, child]) => (
              <Node
                key={key}
                name={key}
                value={child}
                path={`${path}/${key}`}
                depth={depth + 1}
              />
            ))}
            {shown < entries.length ? (
              <li className="py-1 pl-5">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => fold.showMore(path, shown + fold.pageSize)}
                >
                  Show {Math.min(fold.pageSize, entries.length - shown)} more (
                  {entries.length - shown} hidden)
                </Button>
              </li>
            ) : null}
          </ul>
          <span aria-hidden className="pl-5 text-muted-foreground">
            {Array.isArray(value) ? "]" : "}"}
          </span>
        </>
      ) : null}
    </li>
  );
}

function JsonTree({
  value,
  label = "JSON",
  openDepth = 2,
  pageSize = 50,
  copyable = true,
  className,
}: {
  /** A JSON value: what `JSON.parse` returns. */
  value: unknown;
  /** What the value is, for the region and buttons: "Node status". */
  label?: string;
  /** Levels open at first: 1 opens the root, 2 also its children. */
  openDepth?: number;
  /** Items of a long array shown at a time. */
  pageSize?: number;
  /** A button that copies the whole value as pretty-printed JSON. */
  copyable?: boolean;
  className?: string;
}) {
  const [overrides, setOverrides] = React.useState(
    () => new Map<string, boolean>(),
  );
  const [all, setAll] = React.useState<boolean | null>(null);
  const [shown, setShown] = React.useState(() => new Map<string, number>());
  const fold = React.useMemo<Fold>(
    () => ({
      openDepth,
      all,
      overrides,
      pageSize,
      shown,
      toggle: (path, open) => setOverrides(map => new Map(map).set(path, open)),
      showMore: (path, count) => setShown(map => new Map(map).set(path, count)),
    }),
    [openDepth, all, overrides, pageSize, shown],
  );
  const setEverything = (open: boolean) => {
    setAll(open);
    setOverrides(new Map());
  };

  return (
    <div
      data-slot="json-tree"
      className={cn("flex flex-col rounded-xl border bg-card", className)}
    >
      <div className="flex flex-wrap items-center gap-1 border-b px-3 py-2">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => setEverything(true)}
        >
          Expand all
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => setEverything(false)}
        >
          Collapse all
        </Button>
        {copyable ? (
          <CopyButton
            variant="text"
            text={jsonText(value)}
            label={label}
            title={`Copy ${label} as JSON`}
            className="ml-auto"
          >
            Copy JSON
          </CopyButton>
        ) : null}
      </div>
      <div
        role="region"
        aria-label={label}
        tabIndex={0}
        className="overflow-x-auto px-3 py-3 font-mono text-[0.8125rem] leading-relaxed focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring"
      >
        <FoldContext.Provider value={fold}>
          <ul className="min-w-0">
            <Node name={null} value={value} path="" depth={0} />
          </ul>
        </FoldContext.Provider>
      </div>
    </div>
  );
}

export { JsonTree, type Json };
