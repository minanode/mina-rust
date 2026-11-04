/** The first screen: start the node in this tab, or run it yourself, and
 * the steps a start runs, waiting. Nothing heavy loads until the visitor
 * starts it (boot/autostart.ts). A browser that fails the boot's first
 * check is told why here, before Start, and Start is off. */

import * as React from "react";
import { Play, TriangleAlert } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";

import type { BootFailure } from "../boot/failures";
import type { Role } from "../roles/role";
import { BootSteps } from "./boot-steps";
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
      start={
        <Button
          type="button"
          data-action="start-node"
          aria-describedby={problem ? "start-cost start-problem" : "start-cost"}
          onClick={onStart}
          disabled={problem !== null}
        >
          <Play aria-hidden />
          Start the web node in this tab
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
              {problem.hint ? <p>{problem.hint}</p> : null}
            </AlertDescription>
          </Alert>
        ) : null
      }
      steps={<BootSteps boot={null} />}
    />
  );
}
