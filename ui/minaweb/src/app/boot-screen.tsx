/** The boot: the start screen's page with its steps running, and the
 * failure with its cause. Full page until the node is synced; the steps
 * are inside the shell at /start after that. */

import * as React from "react";
import {
  ArrowRight,
  CirclePause,
  CircleStop,
  LoaderCircle,
  RefreshCw,
  TriangleAlert,
} from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";

import type { BootSnapshot } from "../boot/boot";
import { reloadAndStart } from "../boot/autostart";
import { ROLE_LABELS, writeRole, VIEWER } from "../roles/role";
import { bootActions } from "./boot-context";
import { BootSteps } from "./boot-steps";
import { FailureLines } from "./failure-lines";
import { ProducerUnlock } from "./producer-unlock";
import { StartLayout } from "./start-layout";

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
        <FailureLines lines={failure.lines} />
        {failure.hint ? <p>{failure.hint}</p> : null}
        {!waiting ? (
          <div className="mt-2 flex flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="w-fit"
              onClick={() => reloadAndStart()}
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
                  reloadAndStart();
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

/** The whole page while the node starts and syncs: the start screen's,
 * with Start pressed and its steps running. */
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
  const stopped = boot.failure !== null && boot.failure.kind !== "peers";
  return (
    <StartLayout
      running
      // A stopped start leads with why at every width, as a failed check
      // does before Start.
      noticesFirst={stopped}
      role={boot.role}
      // The unlock card and the paused step say the start waits for the
      // passphrase; a failure's alert is the fact once it stopped: the line
      // goes in both.
      lead={boot.unlock || stopped ? null : undefined}
      start={
        <Button
          type="button"
          disabled
          data-started={stopped ? "stopped" : "starting"}
        >
          {/* No motion while the start waits for the passphrase, as the
              step it waits on is paused. */}
          {stopped ? (
            <CircleStop aria-hidden />
          ) : boot.unlock ? (
            <CirclePause aria-hidden />
          ) : (
            <LoaderCircle aria-hidden className="motion-safe:animate-spin" />
          )}
          {stopped
            ? "Stopped"
            : boot.unlock
              ? "Waiting for the passphrase"
              : "Starting in this tab"}
        </Button>
      }
      notices={
        <>
          <BootFailureAlert boot={boot} />
          <ProducerUnlock
            boot={boot}
            onUnlock={bootActions.unlockProducer}
            onSkip={bootActions.startWithoutRole}
          />
        </>
      }
      steps={<BootSteps boot={boot} />}
      after={
        boot.client ? (
          <div className="flex flex-col gap-2">
            <Button
              type="button"
              variant="outline"
              className="w-full sm:w-fit sm:min-w-64"
              onClick={onOpen}
            >
              Open the node's pages now
              <ArrowRight aria-hidden />
            </Button>
            <p className="text-sm text-muted-foreground">
              They open by themselves once the node is synced.
            </p>
          </div>
        ) : null
      }
    />
  );
}
