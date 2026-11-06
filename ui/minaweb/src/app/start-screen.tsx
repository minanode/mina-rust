/** The first screen: start the node in this tab, or run it yourself, and
 * the steps a start runs, waiting. Nothing heavy loads until the visitor
 * starts it (boot/autostart.ts). A browser that fails the boot's first
 * check is told why here, before Start, and Start is off. */

import * as React from "react";
import { CircleStop, Play, TriangleAlert } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";

import type { BootFailure } from "../boot/failures";
import type { Role } from "../roles/role";
import { BootSteps } from "./boot-steps";
import { FailureLines } from "./failure-lines";
import { StartLayout } from "./start-layout";

export function StartScreen({
  role,
  onStart,
  problem = null,
}: {
  role: Role;
  onStart: () => void;
  /** Why this browser can't run the node (boot/failures.ts checkPage). */
  problem?: BootFailure | null;
}) {
  React.useEffect(() => {
    document.title = "Web node · minanode";
    // For the headless checks: the screen waits for Start.
    document.documentElement.dataset.boot = "choose";
  }, []);
  return (
    <StartLayout
      role={role}
      lead={problem ? null : undefined}
      noticesFirst={problem !== null}
      start={
        // A browser that can't run the node: the button says so, as the
        // boot screen's does once a start fails.
        <Button
          type="button"
          data-action="start-node"
          aria-describedby={problem ? "start-problem" : "start-cost"}
          onClick={onStart}
          disabled={problem !== null}
        >
          {problem ? <CircleStop aria-hidden /> : <Play aria-hidden />}
          {problem
            ? problem.cause === "page"
              ? "Can't start on this page"
              : "Can't start in this browser"
            : "Start the web node in this tab"}
        </Button>
      }
      notices={
        problem ? (
          <Alert
            id="start-problem"
            variant="destructive"
            data-failure={problem.kind}
          >
            <TriangleAlert aria-hidden />
            <AlertTitle>{problem.title}</AlertTitle>
            <AlertDescription>
              <p className="wrap-anywhere">{problem.detail}</p>
              <FailureLines lines={problem.lines} />
              {problem.hint ? <p>{problem.hint}</p> : null}
            </AlertDescription>
          </Alert>
        ) : null
      }
      steps={<BootSteps boot={null} blocked={problem !== null} />}
    />
  );
}
