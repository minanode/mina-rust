import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { Store } from '@ngrx/store';
import { combineLatest, map, timer } from 'rxjs';
import { AppSelectors } from '@app/app.state';
import { MinaState } from '@app/app.setup';
import { formatHeight, freshness } from '@shared/helpers/freshness.helper';
import { VerificationService } from './verification.service';
import { VerificationReceiptComponent } from './verification-receipt.component';
import {
  checkLabel,
  receiptLabel,
  shortReceiptHash,
  VerificationSnapshot,
} from './verification.types';
import { updatedSeconds } from './verification-poll';

@Component({
  selector: 'mina-verification-panel',
  imports: [CommonModule, VerificationReceiptComponent],
  inputs: ['compact'],
  styleUrl: './verification.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <section
      class="verification-panel"
      [class.compact]="compact"
      aria-label="Verification"
    >
      @if (!compact) {
        <h2>Verification</h2>
      }
      @if (state$ | async; as state) {
        @if (compact) {
          <div class="summary-row">
            <h2>
              <button
                type="button"
                class="summary-toggle"
                aria-controls="verification-details"
                [attr.aria-expanded]="expanded"
                (click)="expanded = !expanded"
                data-verification-toggle
              >
                <span class="mina-icon chevron" aria-hidden="true"
                  >chevron_right</span
                >
                <span class="title">Verification</span>
                <span class="summary-line" data-verification-summary>
                  @if (state.data; as data) {
                    <span data-verified-count>{{
                      data.verifiedSinceStart | number
                    }}</span>
                    block proof{{
                      data.verifiedSinceStart === 1 ? '' : 's'
                    }}
                    verified {{ state.browser ? 'in this browser' : 'here' }}
                    since start,
                    <span
                      [class.warn]="data.failuresSinceStart > 0"
                      data-failure-count
                      >{{ data.failuresSinceStart | number }} failed</span
                    >{{ summaryTail(data) }}
                  } @else if (state.unsupported) {
                    Not supported by this node
                  } @else if (state.bootstrap) {
                    Verifying blocks as they arrive; receipts appear once synced
                  } @else if (state.error) {
                    Node busy, retrying
                  } @else {
                    Loading receipts…
                  }
                </span>
              </button>
            </h2>
            @if (state.age !== null) {
              <small class="updated secondary" data-verification-updated
                >Updated {{ state.age }} s ago</small
              >
            }
          </div>
        }
        @if (!compact || expanded) {
          <div id="verification-details" class="verification-details">
            <p class="secondary">
              {{
                state.browser
                  ? 'Proofs checked in this browser'
                  : 'Proofs checked by the selected node'
              }}
              @if (state.age !== null && !compact) {
                <small class="updated" data-verification-updated
                  >Updated {{ state.age }} s ago</small
                >
              }
            </p>
            @if (state.unsupported) {
              <p role="status">
                Verification receipts not supported by this node.
              </p>
            } @else if (state.bootstrap) {
              <p role="status" data-verification-bootstrap>
                Verifying blocks as they arrive; receipts appear once synced
              </p>
            }
            @if (state.error && !state.unsupported) {
              <p class="secondary" role="status" data-verification-retrying>
                Node busy, retrying.
                @if (state.unavailable) {
                  Verification receipts unavailable for now.
                }
              </p>
            } @else if (state.loading && !state.bootstrap) {
              <p>Loading local receipts…</p>
            }
            @if (state.data; as data) {
              @if (!compact) {
                <div class="metrics">
                  <div>
                    <span>Block proofs verified since start</span
                    ><strong data-verified-count>{{
                      data.verifiedSinceStart | number
                    }}</strong>
                  </div>
                  <div>
                    <span>Failed checks since start</span
                    ><strong data-failure-count>{{
                      data.failuresSinceStart | number
                    }}</strong>
                  </div>
                  <div>
                    <span>Median recent proof time</span
                    ><strong data-median>{{
                      data.medianProofTimeMs === null
                        ? 'No measurements'
                        : (data.medianProofTimeMs | number: '1.0-1') + ' ms'
                    }}</strong>
                  </div>
                  <div>
                    <span>Last verified block</span
                    ><strong data-last-verified>{{
                      data.lastVerified
                        ? (data.lastVerified.block.height | number)
                        : 'None'
                    }}</strong>
                  </div>
                </div>
              }
              @if (state.tip; as tip) {
                <div class="best-tip-receipt" data-best-tip-receipt>
                  <span class="secondary"
                    >{{ state.bootstrap ? 'Applied tip' : 'Best tip' }} #{{
                      tip.blockHeight
                    }}
                    · {{ shortHash(tip.bestTipHash) }}:</span
                  >
                  <mina-verification-receipt [stateHash]="tip.bestTipHash" />
                </div>
              }
              @if (data.receipts.length) {
                <div class="table-scroll">
                  <table aria-label="Recent verification receipts">
                    <thead>
                      <tr>
                        <th>Block</th>
                        <th>Block proof</th>
                        @if (!compact) {
                          <th>Ledger hash</th>
                        }
                        <th>Check time (UTC)</th>
                        <th>Details</th>
                      </tr>
                    </thead>
                    <tbody>
                      @for (
                        receipt of data.receipts.slice(0, 5);
                        track receipt.block.stateHash
                      ) {
                        <tr data-verification-row>
                          <td>
                            #{{ receipt.block.height | number
                            }}<small class="key">{{
                              shortHash(receipt.block.stateHash)
                            }}</small>
                          </td>
                          <td
                            [class.verified]="
                              receipt.blockProof.status === 'passed'
                            "
                          >
                            {{ label(receipt) }}
                            @if (receipt.blockProof.elapsedMs !== null) {
                              <small
                                >{{
                                  receipt.blockProof.elapsedMs | number: '1.0-1'
                                }}
                                ms</small
                              >
                            }
                          </td>
                          @if (!compact) {
                            <td class="secondary">
                              No separate check recorded
                            </td>
                          }
                          <td>
                            {{
                              receipt.blockProof.checkedAt === null
                                ? 'Not recorded'
                                : (receipt.blockProof.checkedAt
                                  | date: 'HH:mm:ss' : 'UTC')
                            }}
                          </td>
                          <td>
                            <details>
                              <summary>Details</summary>
                              <dl>
                                <dt>State hash</dt>
                                <dd class="key">
                                  {{ receipt.block.stateHash }}
                                </dd>
                                <dt>Network / arrival</dt>
                                <dd>
                                  {{ receipt.network }} / {{ receipt.arrival }}
                                </dd>
                                <dt>Ledger application</dt>
                                <dd>
                                  {{
                                    receipt.applied ? 'Applied' : 'Not applied'
                                  }}
                                </dd>
                                <dt>Delta chain</dt>
                                <dd>{{ check(receipt.deltaChain) }}</dd>
                                <dt>Body reference</dt>
                                <dd>{{ check(receipt.bodyReference) }}</dd>
                                @if (receipt.verifierKey) {
                                  <dt>Verifier SHA-256</dt>
                                  <dd class="key">{{ receipt.verifierKey }}</dd>
                                }
                              </dl>
                            </details>
                          </td>
                        </tr>
                      }
                    </tbody>
                  </table>
                </div>
              } @else if (!state.bootstrap) {
                <p class="secondary">No retained receipts yet.</p>
              }
              <details class="receipt-notes">
                <summary>
                  About these receipts · {{ data.recentFailureCount }} retained
                  failed checks
                </summary>
                <p class="secondary">
                  Showing up to 5 recent blocks. A block proof covers the
                  protocol state; bootstrap ancestry is not individually
                  proof-verified. Ledger application is recorded separately;
                  these receipts do not report an individual ledger-hash check.
                  Counters reset on restart.
                </p>
                @for (
                  failure of data.recentFailures.slice(0, 10);
                  track $index
                ) {
                  <p>
                    Block {{ failure.block.height }} ·
                    {{ failure.check }} failed ·
                    {{ failure.checkedAt | date: 'HH:mm:ss' : 'UTC' }} UTC
                  </p>
                }
              </details>
            }
          </div>
        }
      }
    </section>
  `,
})
export class VerificationPanelComponent {
  readonly state$ = combineLatest([
    inject(VerificationService).state$,
    inject(Store<MinaState>).select(AppSelectors.activeNodeDetails),
    timer(0, 1000),
  ]).pipe(
    map(([state, details]) => {
      const bootstrap =
        details?.status !== 'Offline' &&
        (!details || !freshness(details, Date.now()).synced);
      return {
        ...state,
        bootstrap,
        unavailable:
          state.error && !state.data && !bootstrap && !state.unsupported,
        tip: details?.bestTipHash && details?.blockHeight ? details : null,
        age: updatedSeconds(state.updatedAt, Date.now()),
      };
    }),
  );
  /** Collapsed to a one-line summary that expands to the details. */
  compact = false;
  expanded = false;
  readonly label = receiptLabel;
  readonly check = checkLabel;
  readonly shortHash = shortReceiptHash;

  /** ", median 172 ms, last #578470": the summary's optional parts. */
  summaryTail(data: VerificationSnapshot): string {
    return [
      data.medianProofTimeMs === null
        ? null
        : `median ${Math.round(data.medianProofTimeMs)} ms`,
      data.lastVerified
        ? `last ${formatHeight(data.lastVerified.block.height)}`
        : null,
    ]
      .filter(Boolean)
      .map(part => `, ${part}`)
      .join('');
  }
}
