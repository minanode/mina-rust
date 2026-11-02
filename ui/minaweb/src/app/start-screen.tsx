/** The first screen: start the node in this tab, or run it yourself, and
 * the steps a start runs, waiting. Nothing heavy loads until the visitor
 * starts it (boot/autostart.ts). */

import * as React from "react";
import { Play } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";

import type { Role } from "../roles/role";
import { BootSteps } from "./boot-steps";
import { StartLayout } from "./start-layout";

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
    <StartLayout
      role={role}
      start={
        <Button
          type="button"
          data-action="start-node"
          aria-describedby="start-cost"
          onClick={onStart}
        >
          <Play aria-hidden />
          Start the web node in this tab
        </Button>
      }
      steps={<BootSteps boot={null} />}
    />
  );
}
