/**
 * Sharing with other browsers (MINAWEB-OPT-IN-SHARING), on the Peers page:
 * a switch, off by default. Turning it on first says what it gives away;
 * either way the node starts again through the page's own restart
 * (RestartConfirm, roles/restart.ts), so the tab's key and role cross it.
 * The full explanation is on the docs page.
 */

import * as React from "react";
import { Share2 } from "lucide-react";

import { Card } from "@minanode/design-system/components/card";
import { Signal } from "@minanode/design-system/patterns/signal";
import { cn } from "@minanode/design-system/lib/utils";

import { SHARING_MAX_PEERS, writeSharing } from "../boot/sharing";
import { restartWithKey } from "../roles/restart";
import type { Role } from "../roles/role";
import { RestartConfirm } from "./restart-confirm";

/** A restart that keeps the choice first, and takes it back if the restart
 * doesn't happen (the key couldn't cross, or storage refused). */
export function restartSharing(
  on: boolean,
  restart: typeof restartWithKey = restartWithKey,
): typeof restartWithKey {
  return async (role, key, choice) => {
    writeSharing(on);
    try {
      await restart(role, key, choice);
    } catch (error) {
      try {
        writeSharing(!on);
      } catch {
        // The choice stays for the next start; the visitor asked for it.
      }
      throw error;
    }
  };
}

function Switch({
  checked,
  className,
  ...props
}: Omit<React.ComponentProps<"button">, "role" | "type"> & {
  checked: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      data-state={checked ? "on" : "off"}
      className={cn(
        "relative inline-flex h-6 w-11 shrink-0 cursor-pointer items-center rounded-full border transition-colors duration-150 outline-hidden focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-solid focus-visible:outline-ring disabled:cursor-default disabled:opacity-50",
        checked ? "border-primary bg-primary" : "border-field bg-muted",
        className,
      )}
      {...props}
    >
      <span
        aria-hidden
        className={cn(
          "inline-block size-4.5 rounded-full shadow-sm transition-transform duration-150",
          checked
            ? "translate-x-5.5 bg-primary-foreground"
            : "translate-x-0.5 bg-muted-foreground",
        )}
      />
    </button>
  );
}

const TURN_ON = (
  <>
    <span className="block">The node starts again, sharing. Then:</span>
    <ul className="mt-2 flex list-disc flex-col gap-1 pl-5">
      <li>
        Other visitors, and anyone running a modified page, get your public IP
        address: IPv4, and IPv6 if you have it.
      </li>
      <li>
        It uses your upload: up to about 130 MiB for each tab you help sync,
        then tens of MiB an hour for each browser connected.
      </li>
      <li>You can turn it off here at any time.</li>
    </ul>
  </>
);

const TURN_OFF =
  "The node starts again without sharing. The browsers connected to this tab are dropped.";

export function SharingCard({
  on,
  browsers,
  role,
  restart = restartWithKey,
}: {
  /** The running node shares: it read the choice when it started. */
  on: boolean;
  /** Browsers connected to it now. */
  browsers: number;
  /** The role the node starts with again: this tab's. */
  role: Role;
  /** For tests. */
  restart?: typeof restartWithKey;
}) {
  const [asking, setAsking] = React.useState(false);
  const titleId = React.useId();
  return (
    <Card
      data-sharing={on ? "on" : "off"}
      className="gap-4 px-5 py-5 sm:px-6"
      aria-labelledby={titleId}
      role="region"
    >
      <div className="flex items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-3">
          <span
            aria-hidden
            className={cn(
              "inline-flex size-9 shrink-0 items-center justify-center rounded-lg border bg-background [&_svg]:size-4.5",
              on ? "text-brand-text" : "text-muted-foreground",
            )}
          >
            <Share2 />
          </span>
          <div className="flex min-w-0 flex-col gap-1">
            <h2 id={titleId} className="font-semibold text-foreground">
              Sharing with other browsers
            </h2>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
              <Signal tone={on ? "ok" : "neutral"}>{on ? "On" : "Off"}</Signal>
              <span data-sharing-detail>
                {on
                  ? `${browsers} of ${SHARING_MAX_PEERS} browsers connected`
                  : "Help other visitors sync from this tab."}
              </span>
            </div>
          </div>
        </div>
        <Switch
          checked={on}
          aria-labelledby={titleId}
          disabled={asking}
          data-action="sharing-switch"
          onClick={() => setAsking(true)}
        />
      </div>
      {asking ? (
        <RestartConfirm
          label={
            on ? "Confirm turning sharing off" : "Confirm turning sharing on"
          }
          message={on ? TURN_OFF : TURN_ON}
          confirmLabel={on ? "Turn sharing off" : "Turn sharing on"}
          role={role}
          onCancel={() => setAsking(false)}
          restart={restartSharing(!on, restart)}
        />
      ) : null}
    </Card>
  );
}
