/** The boot's steps as a timeline, with each running step's progress: the
 * start screen, the boot screen and the Start-up page show the same list. */

import { Timeline } from "@minanode/design-system/patterns/timeline";
import { useNow } from "@minanode/design-system/lib/time";

import {
  PEER_WAIT_MS,
  START_PARTS,
  type BootSnapshot,
  type Step,
} from "../boot/boot";
import { formatBytes } from "../boot/failures";
import { pendingSteps, stepFigures } from "../boot/steps";
import { CONFIG } from "../config";
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

/** The boot's steps, the same list before Start (no boot: every step
 * waiting) and while it runs. A waiting step shows its figure. */
export function BootSteps({ boot }: { boot: BootSnapshot | null }) {
  const now = useNow(boot && !boot.synced ? 500 : 60_000);
  const figures = stepFigures(CONFIG);
  return (
    <Timeline
      label="Starting the web node"
      steps={(boot?.steps ?? pendingSteps()).map(step => ({
        label: step.label,
        state: step.state,
        time: duration(step, now),
        detail:
          step.detail ??
          (step.state === "pending" ? figures[step.id] : undefined),
        body: boot && !boot.synced ? stepBody(step, boot, now) : undefined,
        tone: step.state === "done" ? "ok" : undefined,
      }))}
    />
  );
}
