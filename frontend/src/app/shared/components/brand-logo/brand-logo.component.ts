import { ChangeDetectionStrategy, Component, Input } from '@angular/core';

// The minanode logo: the twin-arches mark and the wordmark ("mina" Inter
// semibold, "node" Inter regular and muted, 22 px, letter-spacing -1 px), as
// in ui/design-system/src/patterns/logo.tsx. The wordmark is Inter outlined
// to paths (rendered with satori from @fontsource/inter 400 and 600), so it
// draws at once and the same everywhere, without waiting for or depending on
// the Google Fonts download. Colours follow the theme (body.dark or
// body.light).
const WIDTH = 141;
const HEIGHT = 32;

@Component({
  selector: 'mina-brand-logo',
  templateUrl: './brand-logo.component.html',
  styleUrls: ['./brand-logo.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { role: 'img', 'aria-label': 'minanode' },
})
export class BrandLogoComponent {
  /** Height in pixels; the mark is a square of this size. */
  @Input() size: number = HEIGHT;
  /** The mark alone when false. */
  @Input() wordmark: boolean = true;

  get viewBox(): string {
    return `0 0 ${this.wordmark ? WIDTH : HEIGHT} ${HEIGHT}`;
  }

  get width(): number {
    return (this.size * (this.wordmark ? WIDTH : HEIGHT)) / HEIGHT;
  }

  get height(): number {
    return this.size;
  }
}
