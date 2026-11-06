/** The boot's steps as a timeline, with each running step's progress: the
 * start screen, the boot screen and the Start-up page show the same list. */

import { Measure } from "@minanode/design-system/patterns/amount";
import { Timeline } from "@minanode/design-system/patterns/timeline";
import { useNow } from "@minanode/design-system/lib/time";

import { START_PARTS, type BootSnapshot, type Step } from "../boot/boot";
import { formatBytes } from "../boot/failures";
import { pendingSteps, stepFigures } from "../boot/steps";
import { CONFIG } from "../config";
import { Figures, FrontierStrip, LedgerBars, Parts } from "./sync-graphic";

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

/** A step's time, figures in mono with muted units. */
function stepTime(step: Step, now: number) {
  const d = duration(step, now);
  return d ? <Measure value={d} /> : undefined;
}

function DownloadBar({ boot }: { boot: BootSnapshot }) {
  const d = boot.download;
  if (!d) return null;
  if (d.total === null) {
    return (
      <p className="text-xs text-muted-foreground tabular-nums">
        <Figures text={`${formatBytes(d.received)} so far`} />
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
      <p className="text-xs text-muted-foreground tabular-nums">
        <Figures
          text={`${formatBytes(d.received)} of ${formatBytes(d.total)}, ${pct}%`}
        />
      </p>
    </div>
  );
}

const CONNECT_PARTS = ["Dial the relay", "Handshake", "Connected"] as const;

function ConnectProgress({ boot }: { boot: BootSnapshot }) {
  const c = boot.connect;
  if (!c) return null;
  const part = c.connected > 0 ? 3 : c.connecting > 0 ? 1 : 0;
  return (
    <div className="flex flex-col gap-3">
      <Parts parts={CONNECT_PARTS} current={part} label="Connecting" />
      <p className="text-xs text-muted-foreground tabular-nums">
        <Figures
          text={`${c.relays === 1 ? "1 relay" : `${c.relays} relays`}, ${c.connecting} handshaking, ${c.connected} connected`}
        />
      </p>
    </div>
  );
}

/** What a step shows under its detail while it runs: its progress. */
function stepBody(step: Step, boot: BootSnapshot) {
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
      return current ? <ConnectProgress boot={boot} /> : null;
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

/** The steps before Start: every one waiting, or, when the browser check
 * failed, that step stopped and the rest not reached. */
function beforeStart(blocked: boolean): Step[] {
  return pendingSteps().map(step =>
    !blocked
      ? step
      : { ...step, state: step.id === "check" ? "stopped" : "skipped" },
  );
}

/** Once a step stopped, the ones waiting after it will not come: not
 * reached, with no figure of what they would do. */
function afterStop(steps: Step[]): Step[] {
  const stop = steps.findIndex(s => s.state === "stopped");
  if (stop < 0) return steps;
  return steps.map((step, i) =>
    i > stop && step.state === "pending"
      ? { ...step, state: "skipped", detail: null }
      : step,
  );
}

/** The boot's steps, the same list before Start (no boot) and while it
 * runs. A waiting step shows its figure. While the start waits for the
 * producer key's passphrase it is paused: no time, no progress. */
export function BootSteps({
  boot,
  blocked = false,
}: {
  boot: BootSnapshot | null;
  /** Before Start: the browser check failed. */
  blocked?: boolean;
}) {
  const now = useNow(boot && !boot.synced ? 500 : 60_000);
  const figures = stepFigures(CONFIG);
  return (
    <Timeline
      label="Starting the web node"
      steps={afterStop(boot?.steps ?? beforeStart(blocked)).map(step => {
        const paused =
          boot?.unlock != null &&
          step.id === "start" &&
          step.state === "current";
        return {
          label: step.label,
          state: paused ? "paused" : step.state,
          time: paused ? undefined : stepTime(step, now),
          // Details are fragments, as the waiting figures are: no full stop.
          detail:
            step.detail?.replace(/\.$/, "") ??
            (step.state === "pending" || step.id === "check"
              ? figures[step.id]
              : undefined),
          body:
            boot && !boot.synced && !paused ? stepBody(step, boot) : undefined,
          // The boot stops a step only on a failure.
          tone:
            step.state === "done"
              ? "ok"
              : step.state === "stopped"
                ? "error"
                : undefined,
        };
      })}
    />
  );
}
