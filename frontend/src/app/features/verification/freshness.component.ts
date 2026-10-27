import { CONFIG } from '@shared/constants/config';
import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { Store } from '@ngrx/store';
import { combineLatest, filter, map, timer } from 'rxjs';
import { AppSelectors } from '@app/app.state';
import { MinaState } from '@app/app.setup';
import { AppNodeDetails } from '@shared/types/app/app-node-details.type';
import {
  formatAge,
  formatHeight,
  freshness,
  peerNote,
  shownStatus,
  utcTime,
} from '@shared/helpers/freshness.helper';

/**
 * Block age and data freshness, side by side and labelled (R26c): the best
 * tip block's own time, the node's last sync event, and this page's last
 * status sample. Re-evaluated every second, so a missing sample turns stale.
 */
@Component({
  selector: 'mina-freshness',
  imports: [CommonModule],
  styleUrl: './verification.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (vm$ | async; as vm) {
      <section
        class="verification-panel freshness"
        aria-label="Freshness"
        data-freshness
      >
        <div class="metrics">
          <div data-fresh-tip>
            <span>{{
              vm.synced ? 'Best tip' : 'Applied tip (not synced yet)'
            }}</span>
            <strong>{{ formatHeight(vm.details.blockHeight) }}</strong>
            <small class="secondary"
              >block time {{ utcTime(vm.details.bestTipTimestamp) }}</small
            >
          </div>
          <div data-fresh-tip-age>
            <span>Best tip age</span>
            <strong [class.aware]="vm.f.tipQuiet">{{
              vm.f.tipAgeMs === null ? 'unknown' : formatAge(vm.f.tipAgeMs)
            }}</strong>
            <small class="secondary">now minus the block's timestamp</small>
          </div>
          <div data-fresh-sync>
            <span>Node</span>
            <strong [class.verified]="vm.synced && !vm.f.sampleStale">{{
              vm.statusLabel
            }}</strong>
            <small class="secondary">
              {{
                vm.f.syncEventAgeMs === null
                  ? 'no sync event yet'
                  : (vm.synced ? 'synced ' : 'sync phase changed ') +
                    formatAge(vm.f.syncEventAgeMs) +
                    ' ago'
              }}
            </small>
          </div>
          <div data-fresh-sample>
            <span>Status sampled</span>
            <strong [class.warn]="vm.f.sampleStale">{{
              vm.f.sampleAgeMs === null
                ? 'never'
                : formatAge(vm.f.sampleAgeMs) + ' ago'
            }}</strong>
            <small class="secondary">every 10 s by this page</small>
          </div>
          @if (!publicApi) {
            <div data-fresh-peers>
              <span>Highest peer tip</span>
              <strong>{{ formatHeight(vm.details.maxPeerHeight) }}</strong>
              <small class="secondary">{{ vm.peerNote }}</small>
            </div>
          }
        </div>
        @if (vm.f.sampleStale) {
          <p class="warn" role="alert" data-fresh-stale>
            Stale: no status answer for
            {{
              vm.f.sampleAgeMs === null
                ? 'a while'
                : formatAge(vm.f.sampleAgeMs)
            }}. The values above are the last ones received.
          </p>
        } @else if (vm.f.tipQuiet) {
          <p class="aware" role="status" data-fresh-quiet>
            No new block seen for {{ formatAge(vm.f.tipAgeMs) }}. Mainnet makes
            a block about every 3 minutes on average, with longer gaps at times;
            compare with a block explorer before calling it a fault.
          </p>
        } @else if (!vm.synced) {
          <p class="secondary" role="status">
            Not synced yet{{
              vm.details.targetHeight
                ? ': syncing to ' + formatHeight(vm.details.targetHeight)
                : ''
            }}. Pages show the node's current data, which lags the chain until
            it syncs.
          </p>
        }
      </section>
    }
  `,
})
export class FreshnessComponent {
  readonly publicApi = CONFIG.publicApi;
  readonly formatAge = formatAge;
  readonly formatHeight = formatHeight;
  readonly utcTime = utcTime;
  readonly vm$ = combineLatest([
    inject(Store<MinaState>)
      .select(AppSelectors.activeNodeDetails)
      .pipe(filter(Boolean)),
    timer(0, 1000),
  ]).pipe(
    map(([details]: [AppNodeDetails, number]) => {
      const now = Date.now();
      const f = freshness(details, now);
      return {
        details,
        f,
        synced: f.synced,
        statusLabel: shownStatus(details, now),
        peerNote: peerNote(f),
      };
    }),
  );
}
