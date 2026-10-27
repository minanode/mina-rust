import {
  AfterViewInit,
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  OnInit,
  ViewChild,
} from '@angular/core';
import { untilDestroyed } from '@ngneat/until-destroy';
import { StoreDispatcher } from '@shared/base-classes/store-dispatcher.class';
import { WebNodeService } from '@core/services/web-node.service';
import {
  GlobalErrorHandlerService,
  safelyExecuteInBrowser,
} from '@mina-rust/shared';
import { NgClass } from '@angular/common';
import { Router } from '@angular/router';
import { CONFIG, getFirstFeature } from '@shared/constants/config';
import { animate, style, transition, trigger } from '@angular/animations';
import { catchError, exhaustMap, filter, of, take, timer } from 'rxjs';
import { LoadingSpinnerComponent } from '@shared/loading-spinner/loading-spinner.component';
import { FileProgressHelper } from '@core/helpers/file-progress.helper';
import { freshness } from '@shared/helpers/freshness.helper';
import { freshnessFields } from '@app/app.service';
import * as d3 from 'd3';
import { BrandLogoComponent } from '@shared/components/brand-logo/brand-logo.component';

export enum WebNodeStepStatus {
  DONE,
  LOADING,
  PENDING,
}

export interface WebNodeLoadingStep {
  name: string;
  detail: string;
  status: WebNodeStepStatus;
}

/**
 * Start-up as it happens (R26 gap 4): four real milestones, no timer-driven
 * percentage. The ring shows the download's real byte count, then which of
 * the four steps are done; it reaches the end only when the node reports
 * Synced. Messages give elapsed time, not guesses of time left.
 */
@Component({
  selector: 'mina-web-node-initialization',
  templateUrl: './web-node-initialization.component.html',
  styleUrls: ['./web-node-initialization.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { class: 'flex-column h-100 w-100 align-center' },
  imports: [NgClass, LoadingSpinnerComponent, BrandLogoComponent],
  animations: [
    trigger('messageChange', [
      transition('* => *', [
        style({ opacity: 0, transform: 'translateY(-10px)' }),
        animate(
          '300ms ease-out',
          style({ opacity: 1, transform: 'translateY(0)' }),
        ),
      ]),
    ]),
  ],
})
export class WebNodeInitializationComponent
  extends StoreDispatcher
  implements OnInit, AfterViewInit
{
  protected readonly WebNodeStepStatus = WebNodeStepStatus;
  readonly viewOnly: boolean = !CONFIG.controls?.producer;
  readonly network: string = CONFIG.globalConfig?.webNodeNetwork ?? 'devnet';
  readonly frontendCommit: string = (CONFIG.frontendCommit ?? '').slice(0, 7);
  readonly loading: WebNodeLoadingStep[] = [
    {
      name: 'Download the node',
      detail: '',
      status: WebNodeStepStatus.LOADING,
    },
    { name: 'Start the node', detail: '', status: WebNodeStepStatus.PENDING },
    {
      name: 'Connect to a peer',
      detail: '',
      status: WebNodeStepStatus.PENDING,
    },
    {
      name: 'Sync the chain (ledgers, then blocks)',
      detail: '',
      status: WebNodeStepStatus.PENDING,
    },
  ];
  loadingMessage: string = '';
  /** Continue is offered once connected; the node keeps syncing. */
  ready: boolean = false;
  synced: boolean = false;
  hasError: boolean = false;
  errors: string[] = [];

  private startedAt: number = Date.now();
  private stepStartedAt: number = Date.now();
  private svg: any;
  private progressBar: any;
  private arc: any;
  @ViewChild('progress', { static: true })
  private chartContainer: ElementRef<HTMLDivElement>;

  constructor(
    private errorHandler: GlobalErrorHandlerService,
    private webNodeService: WebNodeService,
    private router: Router,
  ) {
    super();
  }

  ngOnInit(): void {
    if (!this.webNodeService.hasWebNodeConfig()) {
      this.router.navigate([getFirstFeature()]);
      return;
    }
    safelyExecuteInBrowser(() => {
      window.dispatchEvent(new CustomEvent('startWebNode'));
    });
    if (this.webNodeService.isWebNodeLoaded()) {
      // Back on this page with a node already running.
      [0, 1, 2].forEach(i => this.done(i));
      this.loading[3].status = WebNodeStepStatus.LOADING;
      this.ready = true;
    } else {
      this.loadingMessage = 'Downloading the node';
      this.fetchProgress();
      this.checkWebNodeProgress();
      this.fetchPeersInformation();
    }
    this.listenToErrorIssuing();
    this.followSync();
    timer(1000, 1000)
      .pipe(untilDestroyed(this))
      .subscribe(() => this.tick());
  }

  ngAfterViewInit(): void {
    this.buildProgressBar();
    this.render();
  }

  private done(index: number): void {
    this.loading[index].status = WebNodeStepStatus.DONE;
    this.stepStartedAt = Date.now();
  }

  private elapsed(since: number): string {
    return `${Math.round((Date.now() - since) / 1000)} s`;
  }

  /** Elapsed time on the running step, refreshed every second. */
  private tick(): void {
    const running = this.loading.findIndex(
      s => s.status === WebNodeStepStatus.LOADING,
    );
    if (running > 0 && !this.hasError) {
      const elapsed = this.elapsed(this.stepStartedAt);
      this.loading[running].detail =
        running === 3 && this.syncDoing
          ? `${this.syncDoing} · ${elapsed}`
          : elapsed;
      this.detect();
    }
  }

  /** What the sync step is doing, from the node's last status. */
  private syncDoing: string = '';

  private checkWebNodeProgress(): void {
    this.webNodeService.webnodeProgress$
      .pipe(untilDestroyed(this))
      .subscribe((state: string) => {
        if (state === 'Loaded') {
          this.done(0);
          this.loading[0].detail = this.downloadedTotal || 'done';
          this.loading[1].status = WebNodeStepStatus.LOADING;
          this.updateLoadingMessage('Starting the node');
        } else if (state === 'Started') {
          this.done(0);
          this.done(1);
          this.loading[1].detail = `after ${this.elapsed(this.startedAt)}`;
          this.loading[2].status = WebNodeStepStatus.LOADING;
          this.updateLoadingMessage('Looking for a peer');
        } else if (state === 'Connected') {
          [0, 1, 2].forEach(i => this.done(i));
          this.loading[2].detail = `after ${this.elapsed(this.startedAt)}`;
          this.loading[3].status = WebNodeStepStatus.LOADING;
          this.updateLoadingMessage(
            'Connected. Syncing now: continue to the dashboard to watch it.',
          );
          this.ready = true;
        }
        this.render();
        this.detect();
      });
  }

  /** Sync progress from the node itself, every 2 s once it runs. */
  private followSync(): void {
    timer(0, 2000)
      .pipe(
        filter(() => this.webNodeService.started && !this.synced),
        exhaustMap(() =>
          this.webNodeService.status$.pipe(
            take(1),
            catchError(() => of(null)),
          ),
        ),
        untilDestroyed(this),
      )
      .subscribe((status: any) => {
        const sync = status?.transition_frontier?.sync;
        if (!sync || this.loading[3].status === WebNodeStepStatus.PENDING) {
          return;
        }
        // The node says Synced at genesis until a peer's tip arrives: only a
        // plausible tip counts.
        const now = Date.now();
        const synced = freshness(
          {
            status: sync.phase,
            blockHeight: status.transition_frontier.best_tip?.height,
            ...freshnessFields(status, now),
          },
          now,
        ).synced;
        if (synced) {
          this.synced = true;
          this.done(3);
          const height = status.transition_frontier.best_tip?.height;
          this.loading[3].detail = `at #${height} after ${this.elapsed(this.startedAt)}`;
          this.updateLoadingMessage(`Synced at block #${height}`);
        } else {
          const target = sync.target?.height;
          const doing =
            sync.phase === 'Catchup'
              ? 'applying blocks'
              : sync.phase === 'Bootstrap'
                ? 'fetching ledgers'
                : 'waiting for the network’s best tip';
          this.syncDoing = doing + (target ? ` to #${target}` : '');
          this.loading[3].detail = `${this.syncDoing} · ${this.elapsed(this.stepStartedAt)}`;
        }
        this.render();
        this.detect();
      });
  }

  private fetchPeersInformation(): void {
    // peers$ reports the first connected peer as progress 'Connected'.
    timer(0, 1000)
      .pipe(
        filter(
          () =>
            this.webNodeService.started &&
            this.loading[2].status !== WebNodeStepStatus.DONE,
        ),
        exhaustMap(() => this.webNodeService.peers$.pipe(take(1))),
        untilDestroyed(this),
      )
      .subscribe();
  }

  private updateLoadingMessage(message: string): void {
    if (this.hasError) {
      return;
    }
    this.loadingMessage = message;
  }

  private listenToErrorIssuing(): void {
    this.errorHandler.errors$
      .pipe(
        filter(errors => !!errors.length),
        untilDestroyed(this),
      )
      .subscribe((error: string) => {
        this.errors.push(error);
        this.loadingMessage = error;
        this.hasError = true;
        this.markErrorOnD3();
        this.detect();
      });
  }

  private downloadFraction: number = 0;
  private downloadedTotal: string = '';

  private fetchProgress(): void {
    FileProgressHelper.progress$
      .pipe(filter(Boolean), untilDestroyed(this))
      .subscribe(progress => {
        this.downloadFraction = Math.min(1, progress.progress / 100);
        this.downloadedTotal = `${(progress.totalSize / 1e6).toFixed(1)} MB`;
        if (this.loading[0].status !== WebNodeStepStatus.DONE) {
          this.loading[0].detail = `${(progress.downloaded / 1e6).toFixed(1)} of ${(progress.totalSize / 1e6).toFixed(1)} MB`;
        }
        this.render();
        this.detect();
      });
  }

  goToDashboard(): void {
    if (!this.ready) {
      return;
    }
    this.router.navigate([getFirstFeature()]);
  }

  /** The ring: download bytes first, then the steps done out of four. */
  private render(): void {
    if (!this.svg) {
      return;
    }
    const doneSteps = this.loading.filter(
      s => s.status === WebNodeStepStatus.DONE,
    ).length;
    if (doneSteps === 0) {
      const pct = Math.floor(this.downloadFraction * 100);
      this.updateProgressBar(this.downloadFraction / 4, `${pct}`, '%');
    } else {
      this.updateProgressBar(doneSteps / 4, `${doneSteps}/4`, '');
    }
  }

  private buildProgressBar(): void {
    const width = this.chartContainer.nativeElement.offsetWidth;
    const height = this.chartContainer.nativeElement.offsetHeight;
    const barWidth = 4;

    this.svg = d3
      .select(this.chartContainer.nativeElement)
      .append('svg')
      .attr('width', width)
      .attr('height', height);

    this.progressBar = this.svg
      .append('g')
      .attr('transform', `translate(${width / 2}, ${height / 2})`);

    const radius = Math.min(width, height) / 2 - barWidth;

    this.progressBar
      .append('circle')
      .attr('r', radius)
      .attr('fill', 'none')
      .attr('stroke', 'var(--base-tertiary2)')
      .attr('stroke-width', 1);

    this.arc = d3
      .arc()
      .innerRadius(radius - barWidth)
      .outerRadius(radius)
      .startAngle(0)
      .endAngle(0);

    this.progressBar
      .append('path')
      .attr('d', this.arc)
      .attr('opacity', 0.8)
      .attr('fill', 'url(#progress-gradient)');

    const defs = this.svg.append('defs');
    const gradient = defs
      .append('linearGradient')
      .attr('id', 'progress-gradient')
      .attr('x1', '0%')
      .attr('x2', '75%')
      .attr('y1', '25%')
      .attr('y2', '0%');

    gradient.append('stop').attr('offset', '8%').attr('stop-color', '#57d7ff');
    gradient.append('stop').attr('offset', '60%').attr('stop-color', '#fda2ff');
    gradient
      .append('stop')
      .attr('offset', '100%')
      .attr('stop-color', '#ff833d');

    this.progressBar
      .append('text')
      .attr('class', 'value')
      .attr('text-anchor', 'middle')
      .attr('alignment-baseline', 'central')
      .attr('dominant-baseline', 'central')
      .attr('font-size', '40px')
      .attr('fill', 'var(--base-primary)')
      .attr('opacity', 0.8)
      .text('0');
    this.progressBar
      .append('text')
      .attr('class', 'symbol')
      .attr('text-anchor', 'start')
      .attr('alignment-baseline', 'central')
      .attr('dominant-baseline', 'central')
      .attr('font-size', '20px')
      .attr('fill', 'var(--base-tertiary)')
      .attr('opacity', 0.8)
      .attr('dy', '-.3em')
      .text('%');
    this.progressBar
      .append('text')
      .attr('class', 'caption')
      .attr('text-anchor', 'middle')
      .attr('font-size', '13px')
      .attr('fill', 'var(--base-tertiary)')
      .attr('dy', '3.2em')
      .text('downloaded');
  }

  private updateProgressBar(
    fraction: number,
    value: string,
    symbol: string,
  ): void {
    this.arc.endAngle(Math.PI * 2 * Math.min(1, fraction));
    this.progressBar.select('path').attr('d', this.arc);
    this.progressBar.select('.value').text(value);
    // Half the value's width, in the symbol's (half-size) ems.
    const valueWidth = value.length * 0.55 + 0.1;
    this.progressBar
      .select('.symbol')
      .attr('dx', `${valueWidth}em`)
      .text(symbol);
    this.progressBar
      .select('.caption')
      .text(symbol ? 'downloaded' : this.synced ? 'synced' : 'steps done');
  }

  private markErrorOnD3(): void {
    if (!this.progressBar) {
      return;
    }
    this.progressBar.select('path').attr('fill', 'var(--warn-primary)');
    this.progressBar.select('.value').attr('fill', 'var(--warn-primary)');
    this.progressBar.select('.symbol').attr('fill', 'var(--warn-primary)');
    this.progressBar.select('path').attr('opacity', 0.4);
  }
}
