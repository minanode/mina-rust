/**
 * The SNARK worker: before it runs, its fee, the key its fees go to (the
 * key held or saved here by default, or any other), the fee rule for a
 * recipient without an account and what it costs the device;
 * while it runs, the dashboard's worker views (state, recent jobs, the work
 * blocks bought, with the fees). Starting and stopping start the node
 * again (roles/role.ts).
 */

import * as React from "react";
import { Play, Power } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
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
import { looksLikePublicKey, type KeyGlue } from "../roles/keys";
import { VIEWER } from "../roles/role";
import { RestartConfirm } from "./restart-confirm";
import { ACCOUNT_CREATION_FEE } from "../roles/payment";
import { FORM_CARD, OneRole, RoleCost, RolesGate } from "./roles-common";

const POLL_MS = 5_000;
/** The fee a key with an account starts with. */
const DEFAULT_FEE = "0.1";
const MIN_NO_ACCOUNT = formatMina(ACCOUNT_CREATION_FEE)!;
/** The docs section on the fee recipient and the account creation fee. */
export const WORKER_FEE_DOCS_URL =
  "https://minanode.org/docs/snark-worker/#fee-recipient";

export function SnarkWorkerPage() {
  const boot = useBoot();
  const running = boot.role.kind === "worker";
  return (
    <>
      <PageHeader
        eyebrow="Devnet"
        title="SNARK worker"
        aside={
          running ? (
            <Signal tone="ok" pulse>
              On in this tab
            </Signal>
          ) : null
        }
      />
      <RolesGate>
        <div className="flex flex-col gap-6">
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

/** A typed fee recipient: a public key `run` takes (the node's own check,
 * with its checksum). */
export function feeRecipient(
  text: string,
  glue: Pick<KeyGlue, "account_public_key_valid"> | null,
): { recipient: string | null; problem: string | null } {
  const recipient = text.trim();
  if (!recipient)
    return { recipient: null, problem: "Enter the public key the fees go to." };
  return looksLikePublicKey(recipient) &&
    glue?.account_public_key_valid(recipient) === true
    ? { recipient, problem: null }
    : { recipient: null, problem: "Not a public key (B62…)." };
}

function Setup() {
  const boot = useBoot();
  const { key, saved } = useKeys();
  const glue = useKeyGlue();
  // The worker needs no private key: the recipient starts as the key held
  // or saved here, and any public key will do.
  const own = key?.publicKey ?? saved?.publicKey ?? null;
  const [typed, setTyped] = React.useState<string | null>(null);
  const text = typed ?? own ?? "";
  const { recipient, problem: recipientProblem } = feeRecipient(text, glue);
  const account = useAccount(recipient);
  // Known only while the node's last answer stands: a failed lookup, even a
  // refresh after an earlier answer, is no answer.
  const known = account.updatedAt !== null && account.error === null;
  const noAccount = known && account.data === null;
  const [fee, setFee] = React.useState<string | null>(null);
  const shownFee = fee ?? (noAccount ? MIN_NO_ACCOUNT : DEFAULT_FEE);
  const feeNano = parseFee(shownFee);
  const [confirming, setConfirming] = React.useState(false);

  let problem: string | null = null;
  let feeRule = false;
  if (recipientProblem) problem = recipientProblem;
  else if (feeNano === null || feeNano <= 0n)
    problem = "A fee in MINA, more than 0, up to 9 decimals.";
  else if (feeNano < ACCOUNT_CREATION_FEE && !(known && account.data)) {
    feeRule = true;
    problem = account.loading
      ? "Looking up the recipient's account…"
      : known
        ? `This recipient has no account. The fee must be at least ${MIN_NO_ACCOUNT} MINA.`
        : `The recipient's account can't be read yet. The fee must be at least ${MIN_NO_ACCOUNT} MINA.`;
  }

  const whose =
    recipient === null
      ? null
      : recipient === key?.publicKey
        ? "This tab's key"
        : recipient === saved?.publicKey
          ? "Your saved key"
          : null;
  const status =
    recipient === null
      ? // An empty field: the line above Start says what it needs.
        null
      : account.loading
        ? "Looking up its account…"
        : !known
          ? "Its account can't be read yet."
          : account.data
            ? "Has an account on devnet."
            : "No account on devnet yet.";
  const hint = text.trim() && recipientProblem ? recipientProblem : status;

  const switching = boot.role.kind === "producer";
  // Why Start is off, under the field it is about. The button points to it;
  // when the field's own hint already says it, it is for screen readers.
  const problemLine = problem ? (
    <p
      id="worker-problem"
      className={
        // Said already: by the field's own hint, or, for an empty field,
        // by its label. The button points here for screen readers.
        recipientProblem ? "sr-only" : "text-sm text-muted-foreground"
      }
      data-worker-problem
    >
      {problem}
      {feeRule && !account.loading ? (
        <>
          {" "}
          <ExternalLink href={WORKER_FEE_DOCS_URL}>
            SNARK worker docs
          </ExternalLink>
        </>
      ) : null}
    </p>
  ) : null;

  return (
    <div className="grid items-start gap-6 lg:grid-cols-2">
      <Card className={FORM_CARD.card}>
        <CardHeader className={FORM_CARD.header}>
          <CardTitle>Settings</CardTitle>
        </CardHeader>
        <CardContent className={FORM_CARD.content}>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="worker-key">Fee recipient (public key)</Label>
            <Input
              id="worker-key"
              value={text}
              onChange={e => setTyped(e.target.value)}
              placeholder="B62…"
              className="font-mono"
              spellCheck={false}
              autoComplete="off"
              aria-invalid={text.trim() && recipientProblem ? true : undefined}
              aria-describedby="worker-key-hint"
            />
            <p
              id="worker-key-hint"
              className={
                text.trim() && recipientProblem
                  ? "text-sm text-destructive empty:hidden"
                  : "text-sm text-muted-foreground empty:hidden"
              }
              data-fee-rule
              data-recipient-account={
                recipient === null || !known
                  ? undefined
                  : account.data
                    ? "yes"
                    : "no"
              }
            >
              {[whose, hint].filter(Boolean).join(". ")}
            </p>
            {recipientProblem ? problemLine : null}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="worker-fee">Fee per job (MINA)</Label>
            <Input
              id="worker-fee"
              value={shownFee}
              inputMode="decimal"
              className="font-mono tabular-nums"
              onChange={e => setFee(e.target.value)}
            />
            {recipientProblem ? null : problemLine}
          </div>
          {/* Above Start, as on the producer page; the confirm says what
              stops, so the note goes while it asks. */}
          {confirming ? null : <OneRole />}
          {confirming && problem === null && recipient ? (
            <RestartConfirm
              label="Confirm starting the SNARK worker"
              message={
                switching
                  ? "This stops the block producer and starts the node again as a SNARK worker."
                  : "The node restarts as a SNARK worker. Keep this tab open."
              }
              confirmLabel={
                switching
                  ? "Stop producing and start the worker"
                  : "Restart as SNARK worker"
              }
              tone={switching ? "destructive" : "caution"}
              role={{
                kind: "worker",
                publicKey: recipient,
                fee: shownFee.trim(),
              }}
              onCancel={() => setConfirming(false)}
            />
          ) : (
            <Button
              type="button"
              className="w-fit"
              disabled={problem !== null}
              aria-describedby={problem ? "worker-problem" : undefined}
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
  const tip =
    included.data?.bestTipHeight ??
    c?.bestTipHeight ??
    status.data?.bestTip?.height ??
    null;
  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        {confirming ? (
          <RestartConfirm
            label="Confirm stopping the SNARK worker"
            message="The node starts again without the worker. A job in progress is dropped."
            confirmLabel="Stop the worker"
            role={VIEWER}
            tone="destructive"
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
            {/* The state in the Worker card's head, as the Peers cards show
                theirs: beside Stop it read as the stop's progress. */}
            <WorkerStatus
              control={c}
              jobs={status.data?.snarkPool.jobs}
              aside={
                <Signal
                  tone={workerView(c).tone}
                  pulse={workerView(c).label === "Working"}
                  data-worker-state={workerView(c).label}
                >
                  {workerView(c).label}
                </Signal>
              }
            />
            <RoleCost kind="worker" />
          </div>
        )}
      </Loaded>
      {c && c.recent.length > 0 ? <RecentJobs control={c} /> : null}
      {c ? <IncludedSection included={included} tip={tip} /> : null}
      {!synced ? (
        <p className="text-sm text-muted-foreground">
          The worker takes jobs once the node is synced.
        </p>
      ) : null}
    </>
  );
}
