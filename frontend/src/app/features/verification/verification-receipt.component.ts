import {
  ChangeDetectionStrategy,
  Component,
  inject,
  Input,
  OnChanges,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  BehaviorSubject,
  combineLatest,
  filter,
  map,
  switchMap,
  timer,
} from 'rxjs';
import { VerificationService } from './verification.service';
import { updatedSeconds } from './verification-poll';
import {
  checkLabel,
  NO_RETAINED_RECEIPT,
  receiptLabel,
} from './verification.types';

/**
 * The local receipt of one block, looked up by its state hash (R26a), so a
 * block shows its own receipt however many others the node recorded since.
 */
@Component({
  selector: 'mina-verification-receipt',
  imports: [CommonModule],
  styleUrl: './verification.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (view$ | async; as state) {
      @if (state.loading) {
        <span class="secondary">Loading receipt…</span>
      } @else if (state.unsupported && !state.receipt) {
        <span class="secondary"
          >Verification receipts not supported by this node</span
        >
      } @else if (state.error && state.updatedAt === null) {
        <span class="secondary"
          >Verification receipts unavailable · Node busy, retrying</span
        >
      } @else {
        @if (state.error) {
          <small class="secondary">{{
            state.unsupported
              ? 'Not supported by this node'
              : 'Node busy, retrying'
          }}</small>
        }
        @if (state.receipt; as receipt) {
          <span
            data-receipt-label
            [attr.data-receipt-hash]="stateHash"
            [class.verified]="receipt.blockProof.status === 'passed'"
          >
            {{ label(receipt) }}
            @if (receipt.blockProof.elapsedMs !== null) {
              in {{ receipt.blockProof.elapsedMs | number: '1.0-1' }} ms
            }
          </span>
          @if (
            receipt.blockProof.checkedAt &&
            receipt.blockProof.status !== 'pending'
          ) {
            <small
              >{{
                receipt.blockProof.checkedAt | date: 'HH:mm:ss' : 'UTC'
              }}
              UTC</small
            >
          }
          @if (state.age !== null) {
            <small class="secondary updated"
              >Updated {{ state.age }} s ago</small
            >
          }
          @if (detail) {
            <details>
              <summary>Receipt details</summary>
              <p>
                {{
                  state.browser
                    ? 'Checked in this browser'
                    : 'Recorded by the selected node'
                }}
                · {{ receipt.network }} · Arrival: {{ receipt.arrival }}
              </p>
              <dl>
                <dt>Block proof</dt>
                <dd>{{ check(receipt.blockProof) }}</dd>
                <dt>Delta chain proof</dt>
                <dd>{{ check(receipt.deltaChain) }}</dd>
                <dt>Body reference</dt>
                <dd>{{ check(receipt.bodyReference) }}</dd>
              </dl>
              @if (receipt.verifierKey) {
                <p class="key">
                  Verifier index SHA-256: {{ receipt.verifierKey }}
                </p>
              }
            </details>
          }
        } @else if (state.complete) {
          <span
            data-receipt-label
            [attr.data-receipt-hash]="stateHash"
            class="secondary"
            title="This node keeps receipts for the 1,024 blocks it saw most recently, and none for this one."
            >{{ none }}</span
          >
        } @else {
          <span class="secondary">Receipt not found yet</span>
        }
      }
    }
  `,
})
export class VerificationReceiptComponent implements OnChanges {
  @Input({ required: true }) stateHash!: string;
  @Input() detail = false;
  private readonly service = inject(VerificationService);
  private readonly hash$ = new BehaviorSubject<string | null>(null);
  readonly view$ = this.hash$.pipe(
    filter((hash): hash is string => !!hash),
    switchMap(hash =>
      combineLatest([this.service.receipt$(hash), timer(0, 1000)]).pipe(
        map(([state]) => ({
          ...state,
          age: updatedSeconds(state.updatedAt ?? null, Date.now()),
        })),
      ),
    ),
  );
  readonly label = receiptLabel;
  readonly check = checkLabel;
  readonly none = NO_RETAINED_RECEIPT;

  ngOnChanges(): void {
    if (this.stateHash !== this.hash$.value) {
      this.hash$.next(this.stateHash);
    }
  }
}
