/**
 * The SNARK worker: before it runs, its fee, the key its fees go to, the
 * fee rule for a key without an account and what it costs the device;
 * while it runs, the dashboard's worker views (state, recent jobs, the work
 * blocks bought, with the fees). Starting and stopping start the node
 * again (roles/role.ts).
 */

import * as React from "react";
import { Cpu, Play, Power } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Hash } from "@minanode/design-system/patterns/hash";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { Signal } from "@minanode/design-system/patterns/signal";
import { formatMina, minaToNanomina, workerView } from "@minanode/node-client";
import {
  useIncludedWork,
  useNodeStatus,
  useWorkerControl,
} from "@minanode/node-client/react";
import { useNow } from "@minanode/design-system/lib/time";
import { useSyncView } from "@minanode/node-client/react";
import { Loaded } from "minanode-dashboard/components/states";
import {
  IncludedSection,
  RecentJobs,
  WorkerStatus,
} from "minanode-dashboard/pages/worker";

import { useBoot } from "../app/boot-context";
import { useAccount, useKeyGlue, useKeys } from "../roles/hooks";
import { VIEWER } from "../roles/role";
import { RestartConfirm } from "./restart-confirm";
import { ACCOUNT_CREATION_FEE } from "../roles/payment";
import { DevnetOnly, ONE_ROLE, RoleCost, RolesGate } from "./roles-common";

const POLL_MS = 5_000;
/** The fee a key with an account starts with. */
const DEFAULT_FEE = "0.1";
const MIN_NO_ACCOUNT = formatMina(ACCOUNT_CREATION_FEE)!;

export function SnarkWorkerPage() {
  const boot = useBoot();
  const running = boot.role.kind === "worker";
  return (
    <>
      <PageHeader
        eyebrow="devnet role"
        title="SNARK worker"
        description="Proves the transaction SNARKs blocks need, in this tab, for a fee paid to your key when a block buys the work."
        aside={
          running ? (
            <Signal tone="ok" pulse className="text-sm">
              On in this tab
            </Signal>
          ) : null
        }
      />
      <RolesGate>
        <div className="flex flex-col gap-6">
          <DevnetOnly />
          {running ? <Running /> : <Setup />}
        </div>
      </RolesGate>
    </>
  );
}

/** A fee as `run` takes it: MINA, up to nine decimals. */
function parseFee(text: string): bigint | null {
  const t = text.trim();
  if (!/^\d+(\.\d{1,9})?$/.test(t)) return null;
  return minaToNanomina(t);
}

function Setup() {
  const boot = useBoot();
  const { key } = useKeys();
  const glue = useKeyGlue();
  const [typed, setTyped] = React.useState("");
  const receiver = (key?.publicKey ?? typed).trim();
  const validReceiver = glue ? glue.account_public_key_valid(receiver) : false;
  const account = useAccount(validReceiver ? receiver : null);
  const noAccount = !account.loading && account.data === null;
  const [fee, setFee] = React.useState<string | null>(null);
  const shownFee = fee ?? (noAccount ? MIN_NO_ACCOUNT : DEFAULT_FEE);
  const feeNano = parseFee(shownFee);
  const [confirming, setConfirming] = React.useState(false);

  let problem: string | null = null;
  if (!validReceiver) problem = "Enter the public key the fees go to.";
  else if (feeNano === null || feeNano <= 0n)
    problem = "A fee in MINA, more than 0, up to 9 decimals.";
  else if (account.loading) problem = "Looking up the key's account…";
  else if (noAccount && feeNano < ACCOUNT_CREATION_FEE)
    problem = `This key has no account on devnet: blocks buy its work only at ${MIN_NO_ACCOUNT} MINA or more a job.`;

  const switching = boot.role.kind === "producer";

  return (
    <div className="grid items-start gap-6 lg:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Cpu aria-hidden className="size-4 text-brand-text" />
            Start the SNARK worker
          </CardTitle>
          <CardDescription>
            The worker needs only the public key its fees go to: no private key.
            The node starts again with the worker, and the page brings the chain
            back from this browser's store.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {key ? (
            <div className="flex flex-col gap-1.5">
              <span className="text-sm font-medium text-foreground">
                Fees go to this tab's key
              </span>
              <Hash value={key.publicKey} label="fee receiver" />
            </div>
          ) : (
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="worker-key">Fees go to (public key)</Label>
              <Input
                id="worker-key"
                value={typed}
                onChange={e => setTyped(e.target.value)}
                placeholder="B62…"
                className="font-mono"
                spellCheck={false}
                autoComplete="off"
              />
              <p className="text-sm text-muted-foreground">
                Or make a key on the key screen first.
              </p>
            </div>
          )}
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="worker-fee">Fee per job (MINA)</Label>
            <Input
              id="worker-fee"
              value={shownFee}
              inputMode="decimal"
              onChange={e => setFee(e.target.value)}
            />
            <p className="text-sm text-muted-foreground" data-fee-rule>
              A key with no account must ask at least {MIN_NO_ACCOUNT} MINA a
              job: OCaml's SNARK pool refuses less, as the fee pays to create
              the account.{" "}
              {validReceiver && !account.loading
                ? noAccount
                  ? "This key has no account yet."
                  : "This key has an account, so any fee is allowed."
                : null}
            </p>
          </div>
          <p className="text-sm text-muted-foreground">{ONE_ROLE}</p>
          {problem ? (
            <p className="text-sm text-foreground" data-worker-problem>
              {problem}
            </p>
          ) : null}
          {confirming ? (
            <RestartConfirm
              label="Confirm starting the SNARK worker"
              message={
                switching
                  ? "This stops the block producer and starts the node again as a SNARK worker. Producing again needs its key in this tab."
                  : "The node starts again, as a SNARK worker. Keep the tab open while it works."
              }
              confirmLabel={
                switching ? "Stop producing and start the worker" : "Start it"
              }
              role={{
                kind: "worker",
                publicKey: receiver,
                fee: shownFee.trim(),
              }}
              onCancel={() => setConfirming(false)}
            />
          ) : (
            <Button
              type="button"
              className="w-fit"
              disabled={problem !== null}
              data-action="start-worker"
              onClick={() => setConfirming(true)}
            >
              <Play aria-hidden />
              Start the SNARK worker
            </Button>
          )}
        </CardContent>
      </Card>
      <RoleCost kind="worker" />
    </div>
  );
}

function Running() {
  const control = useWorkerControl(POLL_MS);
  const included = useIncludedWork(POLL_MS * 3);
  const { status } = useNodeStatus();
  const synced = useSyncView(useNow()).key === "Synced";
  const [confirming, setConfirming] = React.useState(false);
  const c = control.data;
  const view = c ? workerView(c) : null;
  const tip =
    included.data?.bestTipHeight ??
    c?.bestTipHeight ??
    status.data?.bestTip?.height ??
    null;
  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        {view ? (
          <Signal
            tone={view.tone}
            pulse={view.label === "Working"}
            data-worker-state={view.label}
          >
            {view.label}
          </Signal>
        ) : null}
        {confirming ? (
          <RestartConfirm
            label="Confirm stopping the SNARK worker"
            message="The node starts again without the worker. A job in progress is dropped."
            confirmLabel="Stop the worker"
            role={VIEWER}
            onCancel={() => setConfirming(false)}
          />
        ) : (
          <Button
            type="button"
            variant="outline"
            data-action="stop-worker"
            onClick={() => setConfirming(true)}
          >
            <Power aria-hidden />
            Stop the SNARK worker
          </Button>
        )}
      </div>
      <Loaded polled={control} what="the worker's state" rows={5}>
        {c => (
          <div className="grid items-start gap-6 lg:grid-cols-2">
            <WorkerStatus control={c} jobs={status.data?.snarkPool.jobs} />
            <RoleCost kind="worker" />
          </div>
        )}
      </Loaded>
      {c && c.recent.length > 0 ? <RecentJobs control={c} /> : null}
      {c ? (
        <IncludedSection included={included} tip={tip} market={false} />
      ) : null}
      {!synced ? (
        <p className="text-sm text-muted-foreground">
          The worker takes jobs once the node is synced.
        </p>
      ) : null}
    </>
  );
}
