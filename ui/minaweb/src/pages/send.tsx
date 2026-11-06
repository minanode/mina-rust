/**
 * A payment from the tab's key: the form, its checks, a confirmation,
 * signing in the WASM and submission to this node's pool, which passes it
 * to the relay; then each payment's progress, until a block includes it
 * (with Mina's recommended 15 confirmations) or it fails.
 */

import * as React from "react";
import { Send, TriangleAlert } from "lucide-react";

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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  tableRegion,
} from "@minanode/design-system/components/table";
import { borderlessInSurface } from "@minanode/design-system/lib/surface";
import { cn } from "@minanode/design-system/lib/utils";
import { Amount } from "@minanode/design-system/patterns/amount";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
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
import type { Polled } from "@minanode/node-client/react";

import { useAccount, useKeyGlue, useKeys, useRoleNode } from "../roles/hooks";
import type { Account } from "../roles/node";
import {
  checkPayment,
  CONFIRMATIONS,
  DEFAULT_FEE,
  MEMO_BYTES,
  memoBytes,
  rejectionText,
  type FormErrors,
  type Payment,
  type PaymentForm,
  type Sent,
  type SentStage,
} from "../roles/payment";
import {
  isOpen,
  nonceFor,
  SENT_OPEN_MAX,
  sentStore,
  trackOnce,
  Tracker,
  type NonceState,
} from "../roles/sent";
import { FAUCET_URL, NeedKey, RolesGate } from "./roles-common";

const TRACK_MS = 10_000;

const STAGES: Record<
  SentStage,
  { label: string; tone: "ok" | "warn" | "info" | "neutral" | "error" }
> = {
  // Sent, in no block yet: in the pool, or not seen since a restart.
  pending: { label: "Pending", tone: "info" },
  included: { label: "In a block", tone: "info" },
  confirmed: { label: "Confirmed", tone: "ok" },
  // Failed spent its fee, Refused never left: errors. Dropped may yet be
  // replaced by its twin: a caution.
  failed: { label: "Failed", tone: "error" },
  dropped: { label: "Dropped", tone: "warn" },
  rejected: { label: "Refused", tone: "error" },
};

/** A reviewed payment: what the confirmation shows, and what is signed.
 * `replaces`: it takes a missing payment's nonce. */
type Reviewed = Payment & { from: string; nonce: number; replaces: boolean };

function useSent() {
  return React.useSyncExternalStore(sentStore.subscribe, sentStore.getSnapshot);
}

export function SendPage() {
  const { key } = useKeys();
  // A form page: the form keeps to a measure; the header and the sent
  // list, a table from 768 px, run to the content edge.
  return (
    <div className="flex flex-col gap-6 lg:gap-8">
      <PageHeader eyebrow="Devnet" title="Send" />
      <RolesGate>
        <div className="flex flex-col gap-6">
          <div className="flex max-w-3xl flex-col gap-6">
            {key ? <SendForm /> : <NeedKey what="Sending" />}
          </div>
          <SentList />
        </div>
      </RolesGate>
    </div>
  );
}

const EMPTY: PaymentForm = { to: "", amount: "", fee: DEFAULT_FEE, memo: "" };

/** The sender's account as far as the page knows: read, confirmed absent
 * from the ledger, or not known yet (loading, or the lookup failed). */
type SenderState =
  | { kind: "account"; account: Account }
  | { kind: "absent" }
  | { kind: "unknown"; failed: boolean };

function senderState(polled: Polled<Account | null>): SenderState {
  if (polled.data) return { kind: "account", account: polled.data };
  // An answer of null, with no error since: the ledger has no account.
  if (polled.updatedAt !== null && polled.error === null) {
    return { kind: "absent" };
  }
  return { kind: "unknown", failed: polled.error !== null };
}

type FormNonce = NonceState | { kind: "unknown"; failed: boolean };

/** The next payment's nonce from the sender's account and this tab's list.
 * A key with no account yet starts at 0, to see the pool's refusal; its
 * kept payments count as any account's do. None while the account isn't
 * known. */
function nonceState(
  from: string,
  sender: SenderState,
  sent: readonly Sent[] = sentStore.getSnapshot(),
): FormNonce {
  switch (sender.kind) {
    case "account":
      return nonceFor(sender.account, sent);
    case "absent":
      return nonceFor(
        { publicKey: from, balance: 0n, nonce: 0, delegate: null },
        sent,
      );
    case "unknown":
      return sender;
  }
}

/** The pool's reasons include `name`. */
const refusedAs = (reason: string | null, name: string) =>
  !!reason?.split("; ").some(r => r.replace(/^"|"$/g, "") === name);

/** The nonce a payment can take now, or null while it can't. */
function usable(state: FormNonce): number | null {
  return state.kind === "ready" || state.kind === "missing"
    ? state.nonce
    : null;
}

/** MINA for the form, without digit groups. */
const minaField = (nanomina: bigint) =>
  (formatMina(nanomina) ?? "").replaceAll(",", "");

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
  const [looking, setLooking] = React.useState(false);
  const to = form.to.trim();
  const recipient = useAccount(
    glue && to && glue.account_public_key_valid(to) ? to : null,
  );
  if (!key || !glue || !node) return null;

  const from = senderState(account);
  const sender: Account | null = from.kind === "account" ? from.account : null;
  const nonce = nonceState(key.publicKey, from, sent);
  const nextNonce = usable(nonce);
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

  /** One look at every open payment now, for a kept one that couldn't be
   * checked. */
  async function lookAgain() {
    if (!node || looking) return;
    setLooking(true);
    try {
      await trackOnce(node, sentStore);
    } finally {
      setLooking(false);
    }
  }

  async function send(p: Reviewed) {
    if (!key || !node || p.from !== key.publicKey) return;
    setBusy(true);
    try {
      // A replacement competes with a payment that may be back: look again
      // right before signing.
      if (p.replaces) await trackOnce(node, sentStore);
      // The nonce shown is the nonce signed; if another payment took it
      // meanwhile, or the missing one is back, review again.
      const state = nonceState(key.publicKey, from);
      const nonce = usable(state);
      if (nonce !== p.nonce || (state.kind === "missing") !== p.replaces) {
        setOutcome(
          p.replaces && nonce !== null && state.kind !== "missing"
            ? `The payment at nonce ${formatCount(p.nonce)} is back in the pool: review it again.`
            : "The next nonce changed since the review: review it again.",
        );
        return;
      }
      const receipt = await node.sendPayment(key.privateKey, {
        from: key.publicKey,
        to: p.to,
        amount: p.amount,
        fee: p.fee,
        nonce,
        memo: p.memo,
      });
      if (!receipt.accepted && refusedAs(receipt.rejection, "Duplicate")) {
        // The pool has this very payment: it holds its nonce.
        const held = sentStore.getSnapshot().find(s => s.hash === receipt.hash);
        if (held && isOpen(held)) {
          sentStore.update(receipt.hash, () => ({ check: undefined }));
        } else {
          sentStore.add({
            hash: receipt.hash,
            from: key.publicKey,
            to: p.to,
            amount: p.amount,
            fee: p.fee,
            nonce,
            memo: p.memo,
            sentAt: Date.now(),
            stage: "pending",
            detail: null,
            inclusion: null,
          });
        }
        setOutcome("This node's pool already has this payment.");
        return;
      }
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
        {from.kind === "absent" ? (
          <Alert variant="caution">
            <TriangleAlert aria-hidden />
            <AlertTitle>This key has no devnet MINA yet</AlertTitle>
            <AlertDescription>
              <p>
                The pool refuses a payment from an empty account.{" "}
                {/* The link ends the line: no full stop after its arrow. */}
                <ExternalLink href={FAUCET_URL}>
                  Get devnet MINA from the faucet
                </ExternalLink>
              </p>
            </AlertDescription>
          </Alert>
        ) : sender ? (
          // Facts, not a sentence: each a muted label and its mono value,
          // a line each on a phone.
          <dl className="flex flex-col gap-1 text-sm sm:flex-row sm:flex-wrap sm:gap-x-6">
            <div className="flex items-baseline gap-2">
              <dt className="text-muted-foreground">From</dt>
              <dd>
                <Hash value={key.publicKey} label="sender" />
              </dd>
            </div>
            <div className="flex items-baseline gap-2">
              <dt className="text-muted-foreground">Balance</dt>
              <dd>
                <Amount unit="MINA" value={formatMina(sender.balance)} />
              </dd>
            </div>
            {nextNonce !== null ? (
              <div className="flex items-baseline gap-2">
                <dt className="text-muted-foreground">Next nonce</dt>
                <dd className="font-mono">{formatCount(nextNonce)}</dd>
              </div>
            ) : null}
          </dl>
        ) : null}
        <NonceNotice
          state={nonce}
          looking={looking}
          onLookAgain={() => void lookAgain()}
          onReadAccount={account.refresh}
          onRefill={p => {
            setForm({
              to: p.to,
              amount: minaField(p.amount),
              fee: minaField(p.fee),
              memo: "",
            });
            setReview(null);
            setOutcome(null);
          }}
        />
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
            const state = nonceState(key.publicKey, from);
            const n = usable(state);
            if (p && n !== null) {
              setReview({
                ...p,
                from: key.publicKey,
                nonce: n,
                replaces: state.kind === "missing",
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
              {/* Typed figures in mono, as every shown one. */}
              <Input
                {...field("amount")}
                inputMode="decimal"
                className="font-mono tabular-nums"
              />
              {fieldError("amount")}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="send-fee">Fee (MINA)</Label>
              <Input
                {...field("fee")}
                inputMode="decimal"
                className="font-mono tabular-nums"
              />
              {fieldError("fee")}
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="send-memo">
              Memo{" "}
              <span className="font-normal text-muted-foreground">
                optional
              </span>
            </Label>
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
              // The tint marks it inside the form's card, with no second
              // border (lib/surface).
              className={cn(
                "flex flex-col gap-3 rounded-lg border border-brand-border bg-brand-wash p-4",
                borderlessInSurface,
              )}
            >
              <p className="text-sm text-foreground">
                The node signs exactly this. A sent payment can't be taken back.
              </p>
              <dl
                className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm"
                data-review
              >
                {/* On a phone the key takes the full width under its label,
                    not three ragged lines beside it. */}
                <dt className="text-muted-on-tint max-sm:col-span-2">To</dt>
                <dd
                  className="font-mono wrap-anywhere max-sm:col-span-2 max-sm:mb-1"
                  data-review-to
                >
                  {review.to}
                </dd>
                <dt className="text-muted-on-tint">Amount</dt>
                <dd>
                  <Amount unit="MINA" value={formatMina(review.amount)} />
                </dd>
                <dt className="text-muted-on-tint">Fee</dt>
                <dd>
                  <Amount unit="MINA" value={formatMina(review.fee)} />
                </dd>
                <dt className="text-muted-on-tint">Memo</dt>
                <dd className="wrap-anywhere" data-review-memo>
                  {review.memo ? (
                    <span className="font-mono">
                      {JSON.stringify(review.memo)}
                    </span>
                  ) : (
                    <span className="text-muted-on-tint">None</span>
                  )}
                </dd>
                <dt className="text-muted-on-tint">Nonce</dt>
                <dd className="font-mono">{formatCount(review.nonce)}</dd>
              </dl>
              <div className="flex gap-2">
                {/* The page's most consequential button, at full size. */}
                <Button
                  type="button"
                  disabled={busy}
                  data-action="confirm-send"
                  onClick={() => void send(review)}
                >
                  {busy ? "Signing and sending…" : "Sign and send"}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => setReview(null)}
                >
                  Cancel
                </Button>
              </div>
            </div>
          ) : (
            <Button
              type="submit"
              className="w-fit"
              disabled={nextNonce === null}
            >
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

/** Why the next payment waits, or takes a missing payment's nonce. */
function NonceNotice({
  state,
  looking,
  onLookAgain,
  onReadAccount,
  onRefill,
}: {
  state: FormNonce;
  looking: boolean;
  onLookAgain: () => void;
  onReadAccount: () => void;
  onRefill: (payment: Sent) => void;
}) {
  const retry = (onClick: () => void, busy = false) => (
    <Button
      type="button"
      size="sm"
      variant="outline"
      className="mt-2 w-fit"
      data-action="retry"
      disabled={busy}
      onClick={onClick}
    >
      {busy ? "Checking…" : "Retry"}
    </Button>
  );
  switch (state.kind) {
    case "ready":
      return null;
    case "unknown":
      return state.failed ? (
        <Alert variant="caution" data-nonce="unknown">
          <AlertTitle>Couldn't read this key's account</AlertTitle>
          <AlertDescription>
            <p>Send waits for its nonce.</p>
            {retry(onReadAccount)}
          </AlertDescription>
        </Alert>
      ) : null;
    case "checking":
      return state.failed ? (
        <Alert variant="caution" data-nonce="check-failed">
          <AlertTitle>
            Couldn't check the payment at nonce {formatCount(state.nonce)}
          </AlertTitle>
          <AlertDescription>
            <p>
              It was sent before the restart. Send waits until this node finds
              it in its pool or a block, or finds it in neither.
            </p>
            {retry(onLookAgain, looking)}
          </AlertDescription>
        </Alert>
      ) : (
        <p className="text-sm text-muted-foreground" data-nonce="checking">
          Checking the payment at nonce {formatCount(state.nonce)}, sent before
          the restart.
        </p>
      );
    case "missing":
      return (
        <Alert variant="caution" data-nonce="missing">
          <AlertTitle>
            Nonce {formatCount(state.nonce)}: payment not found
          </AlertTitle>
          <AlertDescription>
            <p>
              Not in this node's pool or a block; other nodes may still have it.
            </p>
            <Button
              type="button"

              variant="outline"
              className="mt-2 w-fit"
              data-action="refill"
              onClick={() => onRefill(state.payment)}
            >
              Send the same payment again
            </Button>
          </AlertDescription>
        </Alert>
      );
    case "full":
      return (
        <p className="text-sm text-muted-foreground" data-nonce="full">
          {formatCount(SENT_OPEN_MAX)} payments wait for a block. Send more once
          blocks include them.
        </p>
      );
    case "exhausted":
      return (
        <p className="text-sm text-muted-foreground" data-nonce="exhausted">
          This account has used every nonce.
        </p>
      );
  }
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
  const views = sent.map(s => ({
    s,
    stage: STAGES[s.stage],
    // A refused payment never left this node: nothing to look up.
    tx:
      s.stage === "rejected" ? null : explorerTransactionUrl("devnet", s.hash),
    block: explorerBlockUrl("devnet", s.inclusion?.height ?? null),
  }));
  const txCell = (v: (typeof views)[number]) =>
    v.tx ? (
      <ExternalLink href={v.tx}>
        <Hash value={v.s.hash} copy={false} />
      </ExternalLink>
    ) : (
      <Hash value={v.s.hash} label="transaction hash" />
    );
  const blockCell = (v: (typeof views)[number]) => {
    const i = v.s.inclusion;
    if (!i) return null;
    return (
      <>
        {v.block ? (
          <ExternalLink href={v.block}>{formatHeight(i.height)}</ExternalLink>
        ) : (
          formatHeight(i.height)
        )}
        {/* "3 of 15" until confirmed, then the count alone. */}
        <span className="ml-3 font-sans text-muted-foreground">
          <span className="font-mono text-foreground">
            {formatCount(i.depth)}
          </span>
          {i.depth >= CONFIRMATIONS ? " on top" : ` of ${CONFIRMATIONS} on top`}
        </span>
      </>
    );
  };
  return (
    <PageSection title="Sent from this tab">
      {/* On a phone a receipt a payment. */}
      <div className="flex flex-col gap-4 md:hidden">
        {views.map(v => (
          <SentReceipt
            key={v.s.hash}
            stage={v.s.stage}
            head={
              // Dot and word, as the same list's table rows and the job
              // cards show a status a row.
              <Signal
                variant="plain"
                tone={v.stage.tone}
                pulse={v.s.stage === "pending"}
              >
                {v.stage.label}
              </Signal>
            }
            aside={<TimeAgo at={v.s.sentAt} />}
            rows={[
              { label: "Transaction", value: txCell(v) },
              { label: "To", value: <Hash value={v.s.to} label="recipient" /> },
            ]}
            figures={[
              {
                label: "Amount",
                value: <Amount unit="MINA" value={formatMina(v.s.amount)} />,
              },
              {
                label: "Fee",
                value: <Amount unit="MINA" value={formatMina(v.s.fee)} />,
              },
              { label: "Nonce", value: formatCount(v.s.nonce) },
            ]}
            block={blockCell(v)}
            footer={v.s.detail}
          />
        ))}
      </div>
      {/* From 768 px a row a payment, and its note under it. */}
      <Table
        className="[&_td]:px-3 [&_td]:align-middle [&_td]:whitespace-nowrap [&_td:first-child]:pl-4 [&_td:last-child]:pr-4 [&_th]:px-3 [&_th:first-child]:pl-4 [&_th:last-child]:pr-4"
        containerProps={{
          ...tableRegion("Sent from this tab"),
          className: "hidden md:block",
        }}
      >
        <TableHeader>
          <TableRow>
            <TableHead>Status</TableHead>
            <TableHead>Transaction</TableHead>
            <TableHead>To</TableHead>
            <TableHead className="text-right">Amount (MINA)</TableHead>
            <TableHead className="text-right">Fee (MINA)</TableHead>
            <TableHead className="text-right">Nonce</TableHead>
            <TableHead>Block</TableHead>
            <TableHead>Sent</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {views.map(v => (
            <React.Fragment key={v.s.hash}>
              <TableRow
                data-sent-row={v.s.stage}
                className={v.s.detail ? "border-b-0" : undefined}
              >
                <TableCell>
                  <Signal
                    variant="plain"
                    tone={v.stage.tone}
                    pulse={v.s.stage === "pending"}
                  >
                    {v.stage.label}
                  </Signal>
                </TableCell>
                <TableCell>{txCell(v)}</TableCell>
                <TableCell>
                  <Hash value={v.s.to} label="recipient" />
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {formatMina(v.s.amount)}
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {formatMina(v.s.fee)}
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">
                  {formatCount(v.s.nonce)}
                </TableCell>
                <TableCell className="font-mono tabular-nums">
                  {blockCell(v) ?? (
                    <span className="font-sans text-muted-foreground">–</span>
                  )}
                </TableCell>
                <TableCell>
                  <TimeAgo at={v.s.sentAt} />
                </TableCell>
              </TableRow>
              {v.s.detail ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={8}
                    className="pt-0 pb-3 text-xs text-pretty whitespace-normal! text-muted-foreground"
                  >
                    {v.s.detail}
                  </TableCell>
                </TableRow>
              ) : null}
            </React.Fragment>
          ))}
        </TableBody>
      </Table>
    </PageSection>
  );
}

/** A sent payment: its stage and age, what it is and who gets it, its
 * figures side by side, the block it is in and how deep, and what the
 * page knows of its fate. Three or four lines, so a list of them scans. */
function SentReceipt({
  stage,
  head,
  aside,
  rows,
  figures,
  block,
  footer,
}: {
  stage: string;
  head: React.ReactNode;
  aside: React.ReactNode;
  rows: { label: string; value: React.ReactNode }[];
  figures: { label: string; value: React.ReactNode }[];
  block: React.ReactNode;
  footer: string | null;
}) {
  const row = "grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-4 py-2.5";
  const value =
    "min-w-0 text-right font-mono text-[0.8125rem] leading-5 break-words text-foreground";
  return (
    <article
      data-sent={stage}
      className="flex flex-col rounded-xl border bg-card text-sm"
    >
      <div className="flex min-h-12 flex-wrap items-center justify-between gap-2 border-b px-4 py-2.5">
        {head}
        {aside}
      </div>
      <dl className="flex flex-col divide-y divide-border/70 px-4 [&_[data-slot=copy-button]]:-my-1">
        {rows.map(r => (
          <div key={r.label} className={row}>
            <dt className="text-muted-foreground">{r.label}</dt>
            <dd className={value}>{r.value}</dd>
          </div>
        ))}
      </dl>
      <dl className="mx-4 grid grid-cols-3 gap-4 border-t border-border/70 py-2.5">
        {figures.map(f => (
          <div key={f.label} className="flex min-w-0 flex-col gap-0.5">
            <dt className="text-xs text-muted-foreground">{f.label}</dt>
            <dd className="font-mono text-[0.8125rem] leading-5 break-words text-foreground">
              {f.value}
            </dd>
          </div>
        ))}
      </dl>
      {block ? (
        <dl className="mx-4 border-t border-border/70">
          <div className={row}>
            <dt className="text-muted-foreground">Block</dt>
            <dd className={value}>{block}</dd>
          </div>
        </dl>
      ) : null}
      {footer ? (
        <p className="border-t px-4 py-3 text-sm text-pretty text-muted-foreground">
          {footer}
        </p>
      ) : null}
    </article>
  );
}
