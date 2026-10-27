import {
  ChangeDetectionStrategy,
  Component,
  EventEmitter,
  Output,
} from '@angular/core';
import {
  SnarkerSettings,
  WebNodeService,
} from '@core/services/web-node.service';
import * as JSZip from 'jszip';
import { ManualDetection, MinaRustSharedModule } from '@mina-rust/shared';
import { animate, style, transition, trigger } from '@angular/animations';
import { CONFIG, nodeSupportsPage } from '@shared/constants/config';
import { Store } from '@ngrx/store';
import { Router } from '@angular/router';
import { take } from 'rxjs';
import { MinaState } from '@app/app.setup';
import { AppActions } from '@app/app.actions';
import { AppSelectors } from '@app/app.state';
import { AppService } from '@app/app.service';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import { BrandLogoComponent } from '@shared/components/brand-logo/brand-logo.component';

@Component({
  selector: 'mina-web-node-file-upload',
  imports: [MinaRustSharedModule, BrandLogoComponent],
  templateUrl: './web-node-file-upload.component.html',
  styleUrl: './web-node-file-upload.component.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { class: 'h-100 w-100 flex-column align-center' },
  animations: [
    trigger('messageChange', [
      transition(':enter', [
        style({ opacity: 0, transform: 'translateY(-10px)' }),
        animate(
          '300ms ease-out',
          style({ opacity: 1, transform: 'translateY(0)' }),
        ),
      ]),
    ]),
  ],
})
export class WebNodeFileUploadComponent extends ManualDetection {
  @Output() startWebNode: EventEmitter<void> = new EventEmitter<void>();
  /** No key upload or producer start in a view-only build (R26g). */
  readonly viewOnly: boolean = !CONFIG.controls?.producer;
  readonly network: string = CONFIG.globalConfig?.webNodeNetwork ?? 'devnet';
  /** P5.3: this build lets the web node sell SNARK work for a public key. */
  readonly snarkerControl: boolean = !!CONFIG.controls?.snarker;
  snarkerKeyError: string | null = null;
  snarkerFeeError: string | null = null;
  private snarkerKey: string | null = null;
  private snarkerFee: string | undefined;
  validFiles: boolean = false;
  error: boolean = false;
  uploadedFileName: string;
  /**
   * A native node whose block explorer this page offers next to the web
   * node (R27 #3): it opens at once, while the web node takes minutes.
   */
  readonly explorerNode: MinaNode | undefined;

  constructor(
    private webnodeService: WebNodeService,
    private store: Store<MinaState>,
    private router: Router,
    appService: AppService,
  ) {
    super();
    this.explorerNode = appService
      .nodes()
      .find(node => !node.isWebNode && nodeSupportsPage(node, 'blocks'));
  }

  openExplorer(): void {
    const node = this.explorerNode;
    this.store.dispatch(AppActions.changeActiveNode({ node }));
    // After the node change's own navigation, which this one replaces.
    this.router.navigate(['/blocks'], { queryParams: { node: node.name } });
  }

  /** The web node starts only as the selected node. */
  private selectWebNode(): void {
    this.store
      .select(AppSelectors.activeNode)
      .pipe(take(1))
      .subscribe(active => {
        const web = CONFIG.configs.find(node => node.isWebNode);
        if (web && !active?.isWebNode) {
          this.store.dispatch(AppActions.changeActiveNode({ node: web }));
        }
      });
  }

  startAutoWebNode(): void {
    this.webnodeService.blockProducerConfig = { mode: 'auto' };
    this.startWebNode.emit();
  }

  onStartDevelopWebnodeBP(): void {
    this.startWebNode.emit();
  }

  onStartDevelopWebnodeNonBP(): void {
    this.selectWebNode();
    this.webnodeService.blockProducerConfig = { mode: 'observer' };
    delete CONFIG.globalConfig.features['block-production'];
    this.startWebNode.emit();
  }

  onFileSelected(event: any): void {
    this.processZipFile(event.target.files[0]).then(files => {
      const publicKey = files.find(f => f.name.endsWith('.pub'))?.content;
      const password = files
        .find(f => f.name.includes('password'))
        ?.content.replace(/\r?\n|\r/g, '');
      const stake = files.find(
        f => !f.name.includes('password') && !f.name.endsWith('.pub'),
      )?.content;
      if (this.error || !publicKey || !stake) {
        this.error = true;
      } else {
        this.webnodeService.blockProducerConfig = {
          mode: 'uploaded',
          data: {
            publicKey,
            password,
            stake: JSON.parse(stake),
          },
        };
        this.validFiles = true;
      }
      this.detect();
    });
  }

  private async processZipFile(
    zipFile: File,
  ): Promise<{ name: string; content: string }[]> {
    const fileContents: { name: string; content: string }[] = [];
    this.uploadedFileName = zipFile.name;

    try {
      const zip = await JSZip.loadAsync(zipFile);
      await Promise.all(
        Object.keys(zip.files).map(async name => {
          if (!zip.files[name].dir) {
            // Skip directories
            try {
              const content = await zip.files[name].async('string'); // Read file as text
              fileContents.push({ name, content });
            } catch (readError) {
              console.error(`Error reading file ${name}:`, readError);
              this.error = true;
            }
          }
        }),
      );
    } catch (error) {
      console.error('Error processing ZIP file:', error);
      this.error = true;
    }
    return fileContents;
  }

  /**
   * The SNARK worker's public key. A private key is cleared at once and
   * never shown: a worker needs only the key that receives its fees.
   */
  onSnarkerKey(input: HTMLInputElement): void {
    const key = input.value.trim();
    this.snarkerKeyError = null;
    this.snarkerKey = null;
    if (key.startsWith('EK')) {
      input.value = '';
      this.snarkerKeyError =
        'That looks like a private key, so it was cleared. A SNARK worker ' +
        'takes only the public key (B62...) that receives its fees.';
    } else if (key && !/^B62[1-9A-HJ-NP-Za-km-z]{52}$/.test(key)) {
      this.snarkerKeyError = 'Enter a public key: B62 and 52 more characters.';
    } else if (key) {
      this.snarkerKey = key;
    }
    this.updateSnarker();
  }

  /** The fee per bundle in MINA; empty for the default, 0.1. */
  onSnarkerFee(input: HTMLInputElement): void {
    const fee = input.value.trim();
    this.snarkerFeeError = null;
    if (fee && !/^(\d+\.?\d{0,9}|\.\d{1,9})$/.test(fee)) {
      this.snarkerFeeError = 'Enter the fee in MINA, like 0.1.';
      this.snarkerFee = undefined;
    } else {
      this.snarkerFee = fee || undefined;
    }
    this.updateSnarker();
  }

  /** A worker key with a producer key: the node runs no worker (P5.3). */
  get snarkerWithProducer(): boolean {
    return !!this.snarkerKey && this.validFiles;
  }

  private updateSnarker(): void {
    const settings: SnarkerSettings | null =
      this.snarkerKey && !this.snarkerFeeError
        ? { publicKey: this.snarkerKey, fee: this.snarkerFee }
        : null;
    this.webnodeService.snarkerConfig = settings;
    this.detect();
  }

  clearFiles(): void {
    this.validFiles = false;
    this.uploadedFileName = null;
    this.webnodeService.blockProducerConfig = { mode: 'auto' };
    this.error = false;
  }
}
