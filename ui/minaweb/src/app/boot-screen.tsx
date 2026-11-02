/** The boot: its steps as a timeline, the download's progress bar, and the
 * failure with its cause. Full page until the node is synced; inside the
 * shell at /start after that. */

import * as React from "react";
import { ArrowRight, RefreshCw, TriangleAlert } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Badge } from "@minanode/design-system/components/badge";
import { Button } from "@minanode/design-system/components/button";
import { Card } from "@minanode/design-system/components/card";
import { Logo } from "@minanode/design-system/patterns/logo";
import { ThemeToggle } from "@minanode/design-system/patterns/theme-toggle";
import { Timeline } from "@minanode/design-system/patterns/timeline";
import { useNow } from "@minanode/design-system/lib/time";

import {
  PEER_WAIT_MS,
  START_PARTS,
  type BootSnapshot,
  type Step,
} from "../boot/boot";
import { formatBytes } from "../boot/failures";
import { CONFIG } from "../config";
import { ROLE_LABELS, writeRole, VIEWER } from "../roles/role";
import { bootActions } from "./boot-context";
import { ProducerUnlock } from "./producer-unlock";
import { FrontierStrip, LedgerBars, Parts } from "./sync-graphic";

function duration(step: Step, now: number): string | undefined {
  if (step.startedAt === null) return undefined;
  const end = step.endedAt ?? (step.state === "current" ? now : null);
  if (end === null) return undefined;
  const ms = Math.max(0, end - step.startedAt);
  return ms < 10_000
    ? `${(ms / 1000).toFixed(1)} s`
    : ms < 120_000
      ? `${Math.round(ms / 1000)} s`
      : `${Math.floor(ms / 60_000)} min ${Math.round((ms % 60_000) / 1000)} s`;
}

function DownloadBar({ boot }: { boot: BootSnapshot }) {
  const d = boot.download;
  if (!d) return null;
  if (d.total === null) {
    return (
      <p className="font-mono text-xs text-muted-foreground tabular-nums">
        {formatBytes(d.received)} so far
      </p>
    );
  }
  const pct = Math.min(100, Math.round((d.received / d.total) * 100));
  return (
    <div className="flex flex-col gap-1.5">
      <div
        role="progressbar"
        aria-label="Download of the node"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        className="h-1.5 overflow-hidden rounded-full bg-muted"
      >
        <div
          className="h-full rounded-full bg-primary transition-[width] duration-200"
          style={{ width: `${pct}%` }}
        />
      </div>
      <p className="font-mono text-xs text-muted-foreground tabular-nums">
        {formatBytes(d.received)} of {formatBytes(d.total)}, {pct}%
      </p>
    </div>
  );
}

const CONNECT_PARTS = ["Dial the relay", "Handshake", "Connected"] as const;

function ConnectProgress({ boot, now }: { boot: BootSnapshot; now: number }) {
  const c = boot.connect;
  if (!c) return null;
  const part = c.connected > 0 ? 3 : c.connecting > 0 ? 1 : 0;
  const waited = Math.max(0, now - c.since);
  const pct = Math.min(100, Math.round((waited / PEER_WAIT_MS) * 100));
  return (
    <div className="flex flex-col gap-3">
      <Parts parts={CONNECT_PARTS} current={part} label="Connecting" />
      <p className="font-mono text-xs text-muted-foreground tabular-nums">
        {c.relays === 1 ? "1 relay" : `${c.relays} relays`}, {c.connecting}{" "}
        handshaking, {c.connected} connected, {Math.round(waited / 1000)} s
      </p>
      {part < 3 ? (
        <div className="flex flex-col gap-1">
          <div
            role="progressbar"
            aria-label="Time waited for a peer"
            aria-valuemin={0}
            aria-valuemax={PEER_WAIT_MS / 1000}
            aria-valuenow={Math.round(waited / 1000)}
            className="h-1 overflow-hidden rounded-full bg-muted"
          >
            <div
              className="h-full rounded-full bg-muted-foreground/40 transition-[width] duration-500"
              style={{ width: `${pct}%` }}
            />
          </div>
          <p className="text-xs text-muted-foreground">
            A peer usually answers in a few seconds. At {PEER_WAIT_MS / 1000} s
            the page says what is in the way.
          </p>
        </div>
      ) : null}
    </div>
  );
}

/** What a step shows under its detail while it runs: its progress. */
function stepBody(step: Step, boot: BootSnapshot, now: number) {
  const current = step.state === "current";
  switch (step.id) {
    case "download":
      return current ? <DownloadBar boot={boot} /> : null;
    case "start":
      return current ? (
        <Parts
          parts={START_PARTS}
          current={boot.startPart}
          label="Starting the node"
        />
      ) : null;
    case "connect":
      return current ? <ConnectProgress boot={boot} now={now} /> : null;
    case "ledgers":
      return current ? <LedgerBars boot={boot} /> : null;
    case "blocks": {
      // The frontier to come shows from the ledgers on.
      const ledgers = boot.steps.find(s => s.id === "ledgers");
      return current || ledgers?.state === "current" ? (
        <FrontierStrip boot={boot} />
      ) : null;
    }
    default:
      return null;
  }
}

export function BootSteps({ boot }: { boot: BootSnapshot }) {
  const now = useNow(boot.synced ? 60_000 : 500);
  return (
    <Timeline
      label="Starting the web node"
      steps={boot.steps.map(step => ({
        label: step.label,
        state: step.state,
        time: duration(step, now),
        detail: step.detail ?? undefined,
        body: boot.synced ? undefined : stepBody(step, boot, now),
        tone: step.state === "done" ? "ok" : undefined,
      }))}
    />
  );
}

export function BootFailureAlert({ boot }: { boot: BootSnapshot }) {
  const failure = boot.failure;
  if (!failure) return null;
  // No peer yet is a warning: the node keeps trying.
  const waiting = failure.kind === "peers";
  return (
    <Alert
      variant={waiting ? "caution" : "destructive"}
      data-failure={failure.kind}
    >
      <TriangleAlert aria-hidden />
      <AlertTitle>{failure.title}</AlertTitle>
      <AlertDescription>
        <p className="wrap-anywhere">{failure.detail}</p>
        {failure.hint ? <p>{failure.hint}</p> : null}
        {!waiting ? (
          <div className="mt-2 flex flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="w-fit"
              onClick={() => window.location.reload()}
            >
              <RefreshCw aria-hidden />
              Reload the page
            </Button>
            {boot.role.kind !== "viewer" ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="w-fit"
                onClick={() => {
                  writeRole(VIEWER);
                  window.location.reload();
                }}
              >
                Start without the {ROLE_LABELS[boot.role.kind].toLowerCase()}
              </Button>
            ) : null}
          </div>
        ) : null}
      </AlertDescription>
    </Alert>
  );
}

/** The whole page while the node starts and syncs. */
export function BootScreen({
  boot,
  onOpen,
}: {
  boot: BootSnapshot;
  onOpen: () => void;
}) {
  React.useEffect(() => {
    document.title = "Starting · Web node · minanode";
  }, []);
  const size = CONFIG.wasm.bytes;
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
          <p className="text-pretty text-muted-foreground">
            The page downloads the node{size ? ` (${formatBytes(size)})` : ""}{" "}
            and starts it here. It connects to the network and checks each new
            block's proof itself.
            {boot.role.kind === "viewer"
              ? " It holds no key until you make or import one."
              : ` This tab runs it as a ${ROLE_LABELS[boot.role.kind].toLowerCase()}.`}
          </p>
        </div>
        <BootFailureAlert boot={boot} />
        <ProducerUnlock
          boot={boot}
          onUnlock={bootActions.unlockProducer}
          onSkip={bootActions.startWithoutRole}
        />
        <Card className="gap-5 p-6">
          <BootSteps boot={boot} />
        </Card>
        {boot.client ? (
          <div className="flex flex-col gap-2">
            <Button
              type="button"
              variant="outline"
              className="w-fit"
              onClick={onOpen}
            >
              Open the node's pages now
              <ArrowRight aria-hidden />
            </Button>
            <p className="text-sm text-muted-foreground">
              They open by themselves once the node is synced.
            </p>
          </div>
        ) : null}
      </main>
      <footer className="px-6 py-5 font-mono text-xs text-muted-foreground">
        {CONFIG.commit ? `build ${CONFIG.commit.slice(0, 12)}` : null}
      </footer>
    </div>
  );
}
