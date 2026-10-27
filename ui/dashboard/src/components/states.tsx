/**
 * The states of a request to the node, composed from design-system patterns
 * with the node client's error kinds: placeholders while loading, the node's
 * own words when it fails, and how old the shown figures are. Node-specific
 * (the wording names node flags), so they stay in the dashboard.
 */

import * as React from "react";
import { RefreshCw, TriangleAlert, Unplug } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { DataAge } from "@minanode/design-system/patterns/data-age";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { LoadingCard } from "@minanode/design-system/patterns/loading-card";
import { describeError, kindOf } from "@minanode/node-client";
import type { Polled } from "@minanode/node-client/react";

import { Code } from "./code";

/** "Updated 3 s ago" for a polled answer, with its refresh button. */
export function PolledAge({
  polled,
  staleAfterMs,
  paused = false,
}: {
  polled: Pick<Polled<unknown>, "updatedAt" | "error" | "refresh" | "loading">;
  staleAfterMs: number;
  paused?: boolean;
}) {
  return (
    <DataAge
      updatedAt={polled.updatedAt}
      loading={polled.loading}
      failed={polled.error !== null}
      staleAfterMs={staleAfterMs}
      paused={paused}
      onRefresh={polled.refresh}
    />
  );
}

/** Why a request has nothing to show, and a way to try again. */
export function RequestFailed({
  error,
  what,
  onRetry,
}: {
  error: unknown;
  /** What was asked for: "the peer list". */
  what: string;
  onRetry?: () => void;
}) {
  const kind = kindOf(error);
  const message = describeError(error);
  if (kind === "absent") {
    return (
      <EmptyState
        icon={<Unplug />}
        title={`This node doesn't serve ${what}`}
        description={
          <p>
            {message} It may be an older version, or run with{" "}
            <Code>--public-graphql</Code>, which serves only public routes.
          </p>
        }
      />
    );
  }
  return (
    <EmptyState
      tone={kind === "offline" ? "destructive" : "caution"}
      icon={kind === "offline" ? <Unplug /> : <TriangleAlert />}
      title={
        kind === "offline"
          ? `No answer from the node for ${what}`
          : `The node couldn't give ${what}`
      }
      description={<p>{message}</p>}
      action={
        onRetry ? (
          <Button type="button" variant="outline" size="sm" onClick={onRetry}>
            <RefreshCw aria-hidden />
            Try again
          </Button>
        ) : null
      }
    />
  );
}

/**
 * The states of a polled request: placeholders first, the failure when
 * there's no data, and the data (kept while later requests fail; the page's
 * age says how old it is). An answer of null reaches `children` too.
 */
export function Loaded<T>({
  polled,
  what,
  rows,
  children,
}: {
  polled: Polled<T>;
  what: string;
  rows?: number;
  children: (data: T) => React.ReactNode;
}) {
  if (polled.data !== null) return <>{children(polled.data)}</>;
  if (polled.error !== null) {
    return (
      <RequestFailed
        error={polled.error}
        what={what}
        onRetry={polled.refresh}
      />
    );
  }
  // The node answered null (no producer, no such block): the page says what
  // that means.
  if (!polled.loading && polled.updatedAt !== null) {
    return <>{children(polled.data as T)}</>;
  }
  return <LoadingCard rows={rows} label={`Loading ${what}`} />;
}
