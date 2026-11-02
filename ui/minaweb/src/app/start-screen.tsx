/** The first screen: start the node in this tab, or run it yourself.
 * Nothing heavy loads until the visitor starts it (boot/autostart.ts). */

import * as React from "react";
import { ArrowUpRight, Play } from "lucide-react";

import { Badge } from "@minanode/design-system/components/badge";
import { Button } from "@minanode/design-system/components/button";
import { Logo } from "@minanode/design-system/patterns/logo";
import { ThemeToggle } from "@minanode/design-system/patterns/theme-toggle";

import { CONFIG } from "../config";
import { buildLabel } from "../version";
import { startCost } from "../roles/resources";
import type { Role } from "../roles/role";

/** The minanode.org page with the archive and its signed sums. */
export const RUN_YOURSELF_URL = "https://minanode.org/docs/run-web-node/";

export function StartScreen({
  role,
  onStart,
}: {
  role: Role;
  onStart: () => void;
}) {
  React.useEffect(() => {
    document.title = "Web node · minanode";
    // For the headless checks: the screen waits for Start.
    document.documentElement.dataset.boot = "choose";
  }, []);
  return (
    <div className="flex min-h-dvh flex-col bg-background">
      <header className="flex items-center justify-between px-6 py-5">
        <Logo />
        <ThemeToggle />
      </header>
      <main className="mx-auto flex w-full max-w-xl flex-1 flex-col gap-6 px-6 pt-6 pb-16">
        <div className="flex flex-col gap-3">
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
          <p className="text-pretty text-muted-foreground" id="start-cost">
            {startCost(role.kind, CONFIG.wasm.bytes)}
          </p>
        </div>
        <div className="flex flex-col gap-3 sm:flex-row">
          <Button
            type="button"
            data-action="start-node"
            aria-describedby="start-cost"
            onClick={onStart}
          >
            <Play aria-hidden />
            Start the web node in this tab
          </Button>
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
      </main>
      <footer className="px-6 py-5 font-mono text-xs text-muted-foreground">
        {buildLabel(CONFIG.commit)}
      </footer>
    </div>
  );
}
