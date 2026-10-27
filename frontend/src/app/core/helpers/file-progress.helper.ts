import { BehaviorSubject } from 'rxjs';
import { safelyExecuteInBrowser } from '@mina-rust/shared';

const WASM_FILE_SIZE = 31556926;

class AssetMonitor {
  readonly downloads: Map<string, any> = new Map();
  readonly progress$: BehaviorSubject<any>;

  constructor(progress$: BehaviorSubject<any>) {
    this.progress$ = progress$;
    safelyExecuteInBrowser(() => {
      this.setupInterceptor();
    });
  }

  private setupInterceptor(): void {
    const originalFetch = window.fetch;
    const self = this;

    window.fetch = async function (resource, options) {
      // Only intercept asset requests (you can modify these extensions as needed)
      const assetExtensions = ['.wasm'];
      const isAsset = assetExtensions.some(ext =>
        resource.toString().toLowerCase().endsWith(ext),
      );

      if (!isAsset) {
        return originalFetch(resource, options);
      }

      const startTime = performance.now();
      const downloadInfo = {
        url: resource.toString(),
        startTime,
        progress: 0,
        totalSize: WASM_FILE_SIZE,
        status: 'pending',
        endTime: 0,
        duration: 0,
      };

      self.downloads.set(resource.toString(), downloadInfo);
      self.emitProgress(downloadInfo);

      let response: Response;
      try {
        response = await originalFetch(resource, options);
      } catch (error) {
        downloadInfo.status = 'error';
        self.emitProgress(downloadInfo);
        throw error;
      }

      // The response goes back at once, so that the browser compiles the
      // WASM while it downloads; a clone is read meanwhile for the progress.
      // Reading the clone to the end first held up the compilation until
      // the whole file was in. Passing the response itself, not a new one
      // made from the clone, keeps its URL, by which Chrome caches the
      // compiled code.
      const reader = response.clone().body.getReader();
      (async () => {
        let receivedLength = 0;
        while (true) {
          const { done, value } = await reader.read();
          if (done) {
            break;
          }
          receivedLength += value.length;
          downloadInfo.progress = Math.min(
            (receivedLength / downloadInfo.totalSize) * 100,
            100,
          );
          self.emitProgress(downloadInfo);
        }
        downloadInfo.status = 'complete';
        downloadInfo.endTime = performance.now();
        downloadInfo.duration = downloadInfo.endTime - downloadInfo.startTime;
        self.emitProgress(downloadInfo);
      })().catch(() => {
        downloadInfo.status = 'error';
        self.emitProgress(downloadInfo);
      });
      return response;
    };
  }

  private emitProgress(downloadInfo: any): void {
    this.progress$.next({
      url: downloadInfo.url,
      progress: downloadInfo.progress.toFixed(2),
      totalSize: downloadInfo.totalSize,
      status: downloadInfo.status,
      duration: downloadInfo.duration,
      startTime: downloadInfo.startTime,
      endTime: downloadInfo.endTime,
      downloaded: (downloadInfo.progress * downloadInfo.totalSize) / 100,
    });
  }
}

export class FileProgressHelper {
  static progress$: BehaviorSubject<any> = new BehaviorSubject<any>(null);

  static initDownloadProgress(): void {
    new AssetMonitor(this.progress$);
  }
}
