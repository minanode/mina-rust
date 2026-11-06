/** The start screen's page, which the boot screen keeps after Start: the
 * same heading, buttons and list of steps in the same places, the steps
 * then running. */

import * as React from "react";
import { ArrowUpRight } from "lucide-react";

import { Badge } from "@minanode/design-system/components/badge";
import { Button } from "@minanode/design-system/components/button";
import { Card } from "@minanode/design-system/components/card";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Logo } from "@minanode/design-system/patterns/logo";
import { ThemeToggle } from "@minanode/design-system/patterns/theme-toggle";
import { cn } from "@minanode/design-system/lib/utils";

import { CONFIG } from "../config";
import { buildLabel } from "../version";
import { startCost } from "../roles/resources";
import type { Role } from "../roles/role";

/** The minanode.org page with the archive and its signed sums. */
export const RUN_YOURSELF_URL = "https://minanode.org/docs/run-web-node/";

/** How the web node starts and connects, on minanode.org. */
export const HOW_IT_WORKS_URL =
  "https://minanode.org/docs/web-node-connections/";

export function StartLayout({
  role,
  lead,
  start,
  notices,
  noticesFirst = false,
  steps,
  after,
  running = false,
}: {
  role: Role;
  /** The line under the title: what starting costs, until the start has
   * moved past it (a passphrase to type), or nothing once it failed, as
   * the alert is the fact then. */
  lead?: React.ReactNode | null;
  /** The start button, or what it became. */
  start: React.ReactNode;
  /** Between the buttons and the steps: a failure, the producer's unlock. */
  notices?: React.ReactNode;
  /** Before Start, a failure comes before the button it disables. */
  noticesFirst?: boolean;
  steps: React.ReactNode;
  after?: React.ReactNode;
  /** The boot is running: on a phone the steps come before the buttons. */
  running?: boolean;
}) {
  return (
    <div className="flex min-h-dvh flex-col bg-background bg-hero-glow">
      <header className="flex h-16 items-center justify-between px-gutter">
        <Logo />
        {/* Its glyph, not its box, on the content's right edge. */}
        <ThemeToggle className="-mr-3 sm:-mr-2" />
      </header>
      <main className="mx-auto grid w-full max-w-xl flex-1 content-start gap-6 px-gutter pt-6 pb-16 lg:max-w-6xl lg:items-start lg:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)] lg:gap-14 lg:pt-[clamp(4rem,18vh,10rem)]">
        {/* One column below 1024 px, in an order that puts what matters
            first: before Start, the buttons; once it runs, the steps. */}
        {/* From 1024 px the left column starts level with the first step. */}
        <div className="contents lg:flex lg:flex-col lg:gap-6 lg:pt-6">
          <div className="order-1 flex flex-col gap-3 lg:order-1">
            <div className="flex items-center gap-2">
              <Badge>{CONFIG.network}</Badge>
              <span className="font-mono text-label text-brand-text uppercase">
                Web node
              </span>
            </div>
            <h1
              id="page-title"
              tabIndex={-1}
              className="text-title text-balance text-foreground outline-none"
            >
              A Mina node in this tab
            </h1>
            {lead !== null ? (
              <p className="text-pretty text-muted-foreground" id="start-cost">
                {lead ?? startCost(role.kind, CONFIG.wasm.bytes)}
              </p>
            ) : null}
          </div>
          {/* The start button keeps its width when it becomes "Starting",
              so the button beside it stays put. */}
          <div
            className={cn(
              "flex flex-col gap-3 sm:flex-row sm:[&>*:first-child]:min-w-64",
              running ? "order-4" : noticesFirst ? "order-3" : "order-2",
              // From 1024 px the column keeps the same order: a failure
              // before Start comes before the button it disables there too.
              noticesFirst ? "lg:order-3" : "lg:order-2",
            )}
          >
            {start}
            <Button asChild variant="outline">
              <a
                href={RUN_YOURSELF_URL}
                target="_blank"
                rel="noopener noreferrer"
                data-action="run-yourself"
              >
                Download and run it yourself
                <ArrowUpRight aria-hidden />
                <span className="sr-only"> (opens in a new tab)</span>
              </a>
            </Button>
          </div>
          <div
            className={cn(
              "flex flex-col gap-3 empty:hidden",
              running || noticesFirst ? "order-2" : "order-3",
              noticesFirst ? "lg:order-2" : "lg:order-3",
            )}
          >
            {notices}
          </div>
          <div className="order-5 empty:hidden lg:order-4">{after}</div>
          <ExternalLink
            href={HOW_IT_WORKS_URL}
            className="order-6 w-fit text-sm lg:order-5"
            data-link="how-it-works"
          >
            How the web node works
          </ExternalLink>
        </div>
        <Card
          className={cn(
            "gap-5 p-6 lg:order-none",
            running ? "order-3" : "order-4",
          )}
        >
          {steps}
        </Card>
      </main>
      <footer className="px-gutter py-5 font-mono text-xs text-muted-foreground">
        {buildLabel(CONFIG.commit)}
      </footer>
    </div>
  );
}
