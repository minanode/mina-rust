import {
  ChangeDetectionStrategy,
  Component,
  EventEmitter,
} from '@angular/core';
import { AppEnvBuild } from '@shared/types/app/app-env-build.type';
import { ManualDetection, MinaJsonViewerComponent } from '@mina-rust/shared';

/**
 * Which builds the page shows: the node that serves the data, named, with
 * its own build from its `/build_env`, and the page's (UI) commit apart from
 * it (R27 #2).
 */
@Component({
  selector: 'mina-env-build-modal',
  imports: [MinaJsonViewerComponent],
  templateUrl: './env-build-modal.component.html',
  styleUrl: './env-build-modal.component.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { class: 'flex-column w-100 bg-surface border-rad-6 border pb-12' },
})
export class EnvBuildModalComponent extends ManualDetection {
  /** The node that serves the page's data, and what kind it is. */
  source: string;
  /** That node's build, or undefined while it has not answered. */
  nodeBuild: AppEnvBuild | undefined;
  frontendCommit: string;
  close: EventEmitter<void> = new EventEmitter<void>();
}
