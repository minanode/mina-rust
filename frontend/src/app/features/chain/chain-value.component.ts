import { ChangeDetectionStrategy, Component, Input } from '@angular/core';
import { RouterLink } from '@angular/router';
import { MinaRustEagerSharedModule } from '@mina-rust/shared';

@Component({
  selector: 'mina-chain-value',
  imports: [RouterLink, MinaRustEagerSharedModule],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <span class="value">
      @if (kind === 'account') {
        <a
          [routerLink]="['/accounts']"
          [queryParams]="{ publicKey: value }"
          queryParamsHandling="merge"
          [title]="value"
          >{{ display }}</a
        >
      } @else if (kind === 'block') {
        <a
          [routerLink]="['/blocks', value]"
          queryParamsHandling="preserve"
          [title]="value"
          >{{ display }}</a
        >
      } @else {
        <span [title]="value">{{ display }}</span>
      }
      <button
        type="button"
        class="copy"
        [copyToClipboard]="value"
        [attr.aria-label]="'Copy ' + value"
        title="Copy full value"
      >
        <span class="mina-icon f-14" aria-hidden="true">content_copy</span>
      </button>
    </span>
  `,
  styles: [
    `
      :host {
        min-width: 0;
      }
      .value {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        max-width: 100%;
      }
      a,
      .value > span {
        font-family: monospace;
        overflow-wrap: anywhere;
        min-width: 0;
      }
      a {
        color: inherit;
        text-decoration: none;
      }
      a:hover {
        text-decoration: underline;
      }
      .copy {
        padding: 2px;
        display: inline-flex;
        flex-shrink: 0;
        background: transparent;
        color: inherit;
      }
    `,
  ],
})
export class ChainValueComponent {
  @Input({ required: true }) value: string;
  @Input() kind: 'account' | 'block' | 'hash' = 'hash';
  @Input() full = false;

  get display(): string {
    return this.full || this.value.length < 18
      ? this.value
      : `${this.value.slice(0, 7)}…${this.value.slice(-5)}`;
  }
}
