import * as React from "react";
import { DatabaseZap } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { Card } from "@minanode/design-system/components/card";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { restoreHeadline } from "@minanode/node-client";
import { useSyncRestore } from "@minanode/node-client/react";

import { BootFailureAlert, BootSteps } from "../app/boot-screen";
import { useBoot } from "../app/boot-context";
import { resetFrontierStore } from "../boot/boot";

/** The frontier store: what it holds, and a reset that deletes it and
 * restarts the node. */
function FrontierStore() {
  const boot = useBoot();
  const restore = useSyncRestore();
  const [confirming, setConfirming] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [failed, setFailed] = React.useState(false);

  return (
    <Card
      className="max-w-xl gap-4 p-6"
      data-frontier-store={boot.frontierStore ? "on" : "off"}
    >
      <h2 className="font-medium text-foreground">Frontier store</h2>
      {boot.frontierStore ? (
        <>
          <p className="text-sm text-pretty text-muted-foreground">
            The node keeps its checked ledgers and blocks in this browser. A
            reload starts from them instead of the network.{" "}
            {boot.frontierStoreReset
              ? "This run started with the store deleted."
              : restore.data
                ? `This run: ${restoreHeadline(restore.data)}.`
                : "This run synced from the network."}
          </p>
          {confirming ? (
            <InlineConfirm
              label="Confirm deleting the frontier store"
              message="The page reloads, deletes the saved chain and syncs from the network again. That takes about a minute."
              confirmLabel="Delete and restart"
              busyLabel="Restarting"
              busy={busy}
              onConfirm={() => {
                setBusy(true);
                if (!resetFrontierStore()) {
                  setBusy(false);
                  setFailed(true);
                }
              }}
              onCancel={() => setConfirming(false)}
            />
          ) : (
            <Button
              type="button"
              variant="outline"
              className="w-fit"
              data-action="reset-store"
              onClick={() => setConfirming(true)}
            >
              <DatabaseZap aria-hidden />
              Delete the saved chain and restart
            </Button>
          )}
          {failed ? (
            <p className="text-sm text-destructive">
              This browser blocks session storage, so the page can't ask for the
              reset.
            </p>
          ) : null}
        </>
      ) : (
        <p className="text-sm text-pretty text-muted-foreground">
          The store is off in this build. A reload syncs from the network again.
        </p>
      )}
    </Card>
  );
}

/** The boot's steps, once the node runs: how long each took. */
export function BootPage() {
  const boot = useBoot();
  return (
    <>
      <PageHeader
        eyebrow="This tab"
        title="Start-up"
        description="How the node started in this tab, step by step, and how long each step took."
      />
      <BootFailureAlert boot={boot} />
      <Card className="max-w-xl gap-5 p-6">
        <BootSteps boot={boot} />
      </Card>
      <FrontierStore />
    </>
  );
}
