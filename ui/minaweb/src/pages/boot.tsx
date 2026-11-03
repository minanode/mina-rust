import * as React from "react";
import { DatabaseZap } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { Card } from "@minanode/design-system/components/card";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { restoreHeadline } from "@minanode/node-client";
import { useNodeStatus, useSyncRestore } from "@minanode/node-client/react";

import { BootFailureAlert } from "../app/boot-screen";
import { BootSteps } from "../app/boot-steps";
import { useBoot } from "../app/boot-context";
import { resetFrontierStore } from "../boot/boot";
import {
  SAVED_CHAIN_DOCS_URL,
  savedChainLabel,
  savedChainReason,
  storeLabel,
  storeState,
} from "../lib/store";

/** The frontier store: its status, how this run started, and a reset that
 * deletes it and restarts the node. */
export function FrontierStoreCard() {
  const boot = useBoot();
  const { status } = useNodeStatus();
  const restore = useSyncRestore();
  const [confirming, setConfirming] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [failed, setFailed] = React.useState(false);
  const store = status.data?.frontierStore;
  const state = storeState(boot.frontierStore, store);
  // The reset only once the node says the store is open in this tab.
  const usable = state.kind === "on";
  const saved = savedChainLabel(state, store?.savedChain);
  const savedReason = savedChainReason(store?.savedChain);

  const rows = [
    { label: "Status", value: storeLabel(state), tone: "brand" as const },
  ];
  if (state.kind === "other-tab") {
    rows.push({ label: "Held by", value: "Another tab", tone: "brand" });
  } else if (state.kind === "unavailable") {
    rows.push({ label: "Reason", value: state.reason, tone: "brand" });
  }
  if (saved) {
    rows.push({ label: "Saved chain", value: saved, tone: "brand" });
    if (savedReason) {
      rows.push({ label: "Why", value: savedReason, tone: "brand" });
    }
  }
  if (usable) {
    rows.push({
      label: "This run",
      value: boot.frontierStoreReset
        ? "Started with the store deleted"
        : restore.data
          ? restoreHeadline(restore.data)
          : "Synced from the network",
      tone: "brand",
    });
  }

  return (
    <KeyValueList
      className="max-w-xl"
      data-frontier-store={state.kind}
      title="Frontier store"
      aside={
        <ExternalLink href={SAVED_CHAIN_DOCS_URL} className="font-sans text-sm">
          Web node docs
        </ExternalLink>
      }
      rows={rows}
      footer={
        usable ? (
          <div className="flex flex-col gap-3">
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
                This browser blocks session storage, so the page can't ask for
                the reset.
              </p>
            ) : null}
          </div>
        ) : null
      }
    />
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
        description="How the node started in this tab."
      />
      <BootFailureAlert boot={boot} />
      <Card className="max-w-xl gap-5 p-6">
        <BootSteps boot={boot} />
      </Card>
      <FrontierStoreCard />
    </>
  );
}
