/**
 * A payment from the tab's key: the form, its checks, a confirmation,
 * signing in the WASM and submission to this node's pool, which passes it
 * to the relay; then each payment's progress, until a block includes it
 * (with Mina's recommended 15 confirmations) or it fails.
 */

import * as React from "react";
import { Send } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Amount } from "@minanode/design-system/patterns/amount";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import { Signal } from "@minanode/design-system/patterns/signal";
import {
  explorerBlockUrl,
  explorerTransactionUrl,
  formatCount,
  formatHeight,
  formatMina,
} from "@minanode/node-client";

import { useAccount, useKeyGlue, useKeys, useRoleNode } from "../roles/hooks";
import type { Account } from "../roles/node";
import {
  checkPayment,
  CONFIRMATIONS,
  DEFAULT_FEE,
  MEMO_BYTES,
  memoBytes,
  nextNonce,
  rejectionText,
  type FormErrors,
  type Payment,
  type PaymentForm,
  type SentStage,
} from "../roles/payment";
import { isOpen, sentStore, Tracker } from "../roles/sent";
import { DevnetOnly, FAUCET_URL, NeedKey, RolesGate } from "./roles-common";

const TRACK_MS = 10_000;

const STAGES: Record<
  SentStage,
  { label: string; tone: "ok" | "warn" | "info" | "neutral" }
> = {
  pending: { label: "In the pool", tone: "info" },
  included: { label: "In a block", tone: "info" },
  confirmed: { label: "Confirmed", tone: "ok" },
  failed: { label: "Failed", tone: "warn" },
  dropped: { label: "Dropped", tone: "warn" },
  rejected: { label: "Refused", tone: "warn" },
};

/** A reviewed payment: what the confirmation shows, and what is signed. */
type Reviewed = Payment & { from: string; nonce: number };

function useSent() {
  return React.useSyncExternalStore(sentStore.subscribe, sentStore.getSnapshot);
}

export function SendPage() {
  const { key } = useKeys();
  return (
    <>
      <PageHeader
        eyebrow="devnet"
        title="Send"
        description="A payment from this tab's key. The node signs it here, in the WASM, and passes it to the network through its relay."
      />
      <RolesGate>
        <div className="flex max-w-3xl flex-col gap-6">
          <DevnetOnly />
          {key ? <SendForm /> : <NeedKey what="Sending" />}
          <SentList />
        </div>
      </RolesGate>
    </>
  );
}

const EMPTY: PaymentForm = { to: "", amount: "", fee: DEFAULT_FEE, memo: "" };

function SendForm() {
  const { key } = useKeys();
  const glue = useKeyGlue();
  const node = useRoleNode();
  const account = useAccount(key?.publicKey ?? null);
  const sent = useSent();
  const [form, setForm] = React.useState<PaymentForm>(EMPTY);
  const [touched, setTouched] = React.useState(false);
  // What the confirmation shows is exactly what is signed: any edit drops
  // it, and Review must be pressed again.
  const [review, setReview] = React.useState<Reviewed | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [outcome, setOutcome] = React.useState<string | null>(null);
  const to = form.to.trim();
  const recipient = useAccount(
    glue && to && glue.account_public_key_valid(to) ? to : null,
  );
  if (!key || !glue || !node) return null;

  const sender: Account | null = account.data ?? null;
  const { payment, errors } = checkPayment(form, {
    from: key.publicKey,
    sender,
    recipient: recipient.loading ? undefined : (recipient.data ?? null),
    validKey: k => glue.account_public_key_valid(k),
  });
  const shown: FormErrors = touched ? errors : {};
  const field = (name: keyof PaymentForm) => ({
    id: `send-${name}`,
    value: form[name],
    onChange: (e: React.ChangeEvent<HTMLInputElement>) => {
      setForm(f => ({ ...f, [name]: e.target.value }));
      setReview(null);
    },
    readOnly: busy,
    "aria-invalid": shown[name] ? true : undefined,
    "aria-describedby": shown[name] ? `send-${name}-error` : undefined,
  });
  const fieldError = (name: keyof PaymentForm) =>
    shown[name] ? (
      <p id={`send-${name}-error`} className="text-sm text-destructive">
        {shown[name]}
      </p>
    ) : null;

  async function send(p: Reviewed) {
    if (!key || !node || p.from !== key.publicKey) return;
    // The nonce shown is the nonce signed; if another payment took it
    // meanwhile, review again.
    const nonce = sender ? nextNonce(sender, sentStore.getSnapshot()) : 0;
    if (nonce !== p.nonce) {
      setReview(null);
      setOutcome("The next nonce changed since the review: review it again.");
      return;
    }
    setBusy(true);
    try {
      const receipt = await node.sendPayment(key.privateKey, {
        from: key.publicKey,
        to: p.to,
        amount: p.amount,
        fee: p.fee,
        nonce,
        memo: p.memo,
      });
      sentStore.add({
        hash: receipt.hash,
        from: key.publicKey,
        to: p.to,
        amount: p.amount,
        fee: p.fee,
        nonce,
        memo: p.memo,
        sentAt: Date.now(),
        stage: receipt.accepted ? "pending" : "rejected",
        detail: receipt.rejection ? rejectionText(receipt.rejection) : null,
        inclusion: null,
      });
      setOutcome(
        receipt.accepted
          ? "Signed and in this node's pool. It goes to the network through the relay."
          : `The pool refused it: ${rejectionText(receipt.rejection ?? "")}`,
      );
      if (receipt.accepted) {
        setForm(EMPTY);
        setTouched(false);
      }
    } catch (e) {
      setOutcome(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
      setReview(null);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>New payment</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {!account.loading && !sender ? (
          <Alert variant="caution">
            <AlertTitle>This key has no devnet MINA yet</AlertTitle>
            <AlertDescription>
              <p>
                <ExternalLink href={FAUCET_URL}>
                  Get devnet MINA from the faucet
                </ExternalLink>{" "}
                for <Hash value={key.publicKey} label="public key" />, then send
                once a block includes it. You can still try: the pool will
                refuse it.
              </p>
            </AlertDescription>
          </Alert>
        ) : sender ? (
          <p className="text-sm text-muted-foreground">
            From <Hash value={key.publicKey} label="sender" />, balance{" "}
            <Amount unit="MINA" value={formatMina(sender.balance)} />, next
            nonce {formatCount(nextNonce(sender, sent))}.
          </p>
        ) : null}
        <form
          className="flex flex-col gap-4"
          data-send-form
          onSubmit={event => {
            event.preventDefault();
            setTouched(true);
            setOutcome(null);
            // An unfunded key may still send, to see the pool's refusal.
            const onlyUnfunded =
              Object.keys(errors).length === 1 && errors.form && !sender;
            const p =
              payment ??
              (onlyUnfunded
                ? checkPayment(form, {
                    from: key.publicKey,
                    sender: {
                      publicKey: key.publicKey,
                      balance: 2n ** 63n,
                      nonce: 0,
                      delegate: null,
                    },
                    recipient: recipient.data ?? null,
                    validKey: k => glue.account_public_key_valid(k),
                  }).payment
                : null);
            if (p) {
              setReview({
                ...p,
                from: key.publicKey,
                nonce: sender ? nextNonce(sender, sentStore.getSnapshot()) : 0,
              });
            }
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="send-to">To</Label>
            <Input
              {...field("to")}
              placeholder="B62…"
              autoComplete="off"
              spellCheck={false}
              className="font-mono"
            />
            {fieldError("to")}
            {recipient.data === null &&
            !recipient.loading &&
            to &&
            !errors.to ? (
              <p className="text-sm text-muted-foreground">
                No account yet: 1 MINA of the amount creates it.
              </p>
            ) : null}
          </div>
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="send-amount">Amount (MINA)</Label>
              <Input {...field("amount")} inputMode="decimal" placeholder="1" />
              {fieldError("amount")}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="send-fee">Fee (MINA)</Label>
              <Input {...field("fee")} inputMode="decimal" />
              {fieldError("fee")}
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="send-memo">Memo (optional)</Label>
            <Input {...field("memo")} autoComplete="off" />
            <p className="text-sm text-muted-foreground">
              {memoBytes(form.memo)} of {MEMO_BYTES} bytes. Memos are public.
            </p>
            {fieldError("memo")}
          </div>
          {shown.form ? (
            <p className="text-sm text-muted-foreground">{shown.form}</p>
          ) : null}
          {review ? (
            <div
              role="group"
              aria-label="Confirm the payment"
              className="flex flex-col gap-3 rounded-lg border border-brand-border bg-brand-subtle p-4"
            >
              <p className="text-sm text-foreground">
                The node signs exactly this. A sent payment can't be taken back.
              </p>
              <dl
                className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm"
                data-review
              >
                <dt className="text-muted-foreground">To</dt>
                <dd className="font-mono wrap-anywhere" data-review-to>
                  {review.to}
                </dd>
                <dt className="text-muted-foreground">Amount</dt>
                <dd>
                  <Amount unit="MINA" value={formatMina(review.amount)} />
                </dd>
                <dt className="text-muted-foreground">Fee</dt>
                <dd>
                  <Amount unit="MINA" value={formatMina(review.fee)} />
                </dd>
                <dt className="text-muted-foreground">Memo</dt>
                <dd className="wrap-anywhere" data-review-memo>
                  {review.memo ? (
                    <span className="font-mono">
                      {JSON.stringify(review.memo)}
                    </span>
                  ) : (
                    <span className="text-muted-foreground">None</span>
                  )}
                </dd>
                <dt className="text-muted-foreground">Nonce</dt>
                <dd className="font-mono">{formatCount(review.nonce)}</dd>
              </dl>
              <div className="flex gap-2">
                <Button
                  type="button"
                  size="sm"
                  disabled={busy}
                  data-action="confirm-send"
                  onClick={() => void send(review)}
                >
                  {busy ? "Signing and sending…" : "Sign and send"}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => setReview(null)}
                >
                  Cancel
                </Button>
              </div>
            </div>
          ) : (
            <Button type="submit" className="w-fit">
              <Send aria-hidden />
              Review
            </Button>
          )}
          {outcome ? (
            <p className="text-sm text-foreground" data-send-outcome>
              {outcome}
            </p>
          ) : null}
        </form>
      </CardContent>
    </Card>
  );
}

/** Follows each sent payment until it is confirmed, fails or drops: one
 * look at a time, each merged by hash into the list as it is then. */
function useTracking() {
  const node = useRoleNode();
  const sent = useSent();
  const open = sent.some(isOpen);
  const tracker = React.useMemo(
    () => (node ? new Tracker(node, sentStore) : null),
    [node],
  );
  React.useEffect(() => {
    if (!tracker || !open) return;
    const timer = setInterval(() => void tracker.tick(), TRACK_MS);
    void tracker.tick();
    return () => clearInterval(timer);
  }, [tracker, open]);
}

function SentList() {
  useTracking();
  const sent = useSent();
  if (sent.length === 0) return null;
  return (
    <PageSection
      title="Sent from this tab"
      description={`Newest first. A payment is confirmed once ${CONFIRMATIONS} blocks are on top of its block.`}
    >
      <div className="flex flex-col gap-4">
        {sent.map(s => {
          const stage = STAGES[s.stage];
          // A refused payment never left this node: nothing to look up.
          const tx =
            s.stage === "rejected"
              ? null
              : explorerTransactionUrl("devnet", s.hash);
          const block = explorerBlockUrl("devnet", s.inclusion?.height ?? null);
          return (
            <KeyValueList
              key={s.hash}
              data-sent={s.stage}
              title={
                <span className="inline-flex items-center gap-2">
                  <Signal tone={stage.tone} pulse={s.stage === "pending"}>
                    {stage.label}
                  </Signal>
                </span>
              }
              aside={<TimeAgo at={s.sentAt} />}
              rows={[
                {
                  label: "Transaction",
                  value: tx ? (
                    <ExternalLink href={tx}>
                      <Hash value={s.hash} copy={false} />
                    </ExternalLink>
                  ) : (
                    <Hash value={s.hash} label="transaction hash" />
                  ),
                },
                { label: "To", value: <Hash value={s.to} label="recipient" /> },
                {
                  label: "Amount",
                  value: <Amount unit="MINA" value={formatMina(s.amount)} />,
                },
                {
                  label: "Fee",
                  value: <Amount unit="MINA" value={formatMina(s.fee)} />,
                },
                { label: "Nonce", value: formatCount(s.nonce) },
                ...(s.inclusion
                  ? [
                      {
                        label: "Block",
                        value: block ? (
                          <ExternalLink href={block}>
                            {formatHeight(s.inclusion.height)}
                          </ExternalLink>
                        ) : (
                          formatHeight(s.inclusion.height)
                        ),
                      },
                      {
                        label: "Blocks on top",
                        value: `${formatCount(s.inclusion.depth)} of ${CONFIRMATIONS}`,
                      },
                    ]
                  : []),
              ]}
              footer={s.detail ?? undefined}
            />
          );
        })}
      </div>
    </PageSection>
  );
}
