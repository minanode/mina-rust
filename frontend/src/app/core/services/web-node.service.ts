import { Injectable } from '@angular/core';
import {
  BehaviorSubject,
  catchError,
  EMPTY,
  filter,
  from,
  fromEvent,
  map,
  merge,
  Observable,
  of,
  shareReplay,
  switchMap,
  take,
  tap,
  throwError,
  timer,
} from 'rxjs';
import base from 'base-x';
import {
  any,
  isBrowser,
  safelyExecuteInBrowser,
  getLocalStorage,
} from '@mina-rust/shared';
import { DashboardPeerStatus } from '@shared/types/dashboard/dashboard.peer';
import { FileProgressHelper } from '@core/helpers/file-progress.helper';
import { CONFIG } from '@shared/constants/config';
import { loadIceServers } from '@shared/helpers/ice-servers.helper';
import {
  fetchPinnedWasm,
  parsePinnedWasm,
} from '@shared/helpers/wasm-integrity.helper';

export interface PrivateStake {
  publicKey: string;
  password: string | null;
  stake: string;
}

export type BlockProducerConfig =
  | { mode: 'observer' }
  | { mode: 'uploaded'; data: PrivateStake }
  | { mode: 'auto' };

/**
 * A SNARK worker for the web node (P5.3): the public key that receives the
 * fees, and the fee per bundle in MINA (default 0.1). No private key.
 */
export type SnarkerSettings = { publicKey: string; fee?: string };

export type WebNodeConfiguration = {
  network: string;
  blockProducer: {
    publicKey: string;
    privateKey: string | [string, string] | null;
  };
  snarker?: SnarkerSettings;
};

@Injectable({
  providedIn: 'root',
})
export class WebNodeService {
  private readonly webnode$: BehaviorSubject<any> = new BehaviorSubject<any>(
    null,
  );
  private readonly wasm$: BehaviorSubject<any> = new BehaviorSubject<any>(null);

  private webNodeStartTime: number;
  private firstPeerConnected: boolean = false;

  readonly webnodeProgress$: BehaviorSubject<string> =
    new BehaviorSubject<string>('');

  memory: WebAssembly.MemoryDescriptor;
  blockProducerConfig: BlockProducerConfig = { mode: 'auto' };
  /** Set on the start page, if the build allows a SNARK worker (P5.3). */
  snarkerConfig: SnarkerSettings | null = null;

  constructor() {
    FileProgressHelper.initDownloadProgress();
    const basex = base(
      '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz',
    );
    safelyExecuteInBrowser(() => {
      any(window).bs58btc = {
        encode: (buffer: Uint8Array | number[]) => 'z' + basex.encode(buffer),
        decode: (string: string) => basex.decode(string.substring(1)),
      };
    });
  }

  get publicKey(): string {
    return this.blockProducerConfig.mode === 'uploaded'
      ? this.blockProducerConfig.data.publicKey
      : undefined;
  }

  hasWebNodeConfig(): boolean {
    return CONFIG.configs.some(c => c.isWebNode);
  }

  /** The node is running and answers RPCs. */
  get started(): boolean {
    return !!this.webnode$.value;
  }

  isWebNodeLoaded(): boolean {
    if (isBrowser()) {
      return !!any(window).webnode;
    }
    return false;
  }

  private start$: Observable<any> | null = null;

  /**
   * Load and start the node once, however many times the browser node is
   * selected: a second start would run a second node (R28). It starts when
   * the module is loaded, which the start page triggers.
   */
  startOnce$(): Observable<any> {
    if (!this.start$) {
      this.start$ = this.loadWasm$().pipe(
        take(1),
        switchMap(cfg => this.startWasm$(cfg)),
        shareReplay(1),
      );
    }
    return this.start$;
  }

  loadWasm$(): Observable<WebNodeConfiguration> {
    this.webNodeStartTime = Date.now();

    if (isBrowser()) {
      return merge(
        of(any(window).webnode).pipe(filter(Boolean)),
        fromEvent(window, 'webNodeLoaded'),
      ).pipe(switchMap(() => this.getWebNodeConfiguration$()));
    }
    return EMPTY;
  }

  /**
   * The SNARK worker to run, if the build allows one (P5.3): the start
   * page's settings, else those of the `a` URL parameter. A public key
   * only; the node refuses anything else without repeating it.
   */
  private snarker(args?: any): SnarkerSettings | undefined {
    if (!CONFIG.controls?.snarker) {
      return undefined;
    }
    const settings = this.snarkerConfig ?? args?.snarker;
    const publicKey = String(settings?.publicKey ?? '').trim();
    if (!publicKey) {
      return undefined;
    }
    const fee = String(settings?.fee ?? '').trim();
    return { publicKey, fee: fee || undefined };
  }

  private getWebNodeConfiguration$(): Observable<WebNodeConfiguration> {
    const DEFAULT_NETWORK = CONFIG.globalConfig?.webNodeNetwork ?? 'devnet';

    if (!CONFIG.controls?.producer) {
      // View-only build: never take a key, from an upload or from the `a`
      // URL parameter. A SNARK worker takes none.
      console.log('WebNode: view-only build, observer mode');
      return of({
        network: DEFAULT_NETWORK,
        blockProducer: { publicKey: '', privateKey: null },
        snarker: this.snarker(this.getWebnodeArgsFromStorage()),
      });
    }

    switch (this.blockProducerConfig.mode) {
      case 'uploaded':
        console.log('WebNode: Using uploaded key configuration');
        return of({
          network: DEFAULT_NETWORK,
          blockProducer: {
            publicKey: this.blockProducerConfig.data.publicKey,
            privateKey: this.blockProducerConfig.data.password
              ? [
                  this.blockProducerConfig.data.stake,
                  this.blockProducerConfig.data.password,
                ]
              : this.blockProducerConfig.data.stake,
          },
          snarker: this.snarker(),
        });

      case 'observer':
        console.log('WebNode: Running in observer mode (no block production)');
        return of({
          network: DEFAULT_NETWORK,
          blockProducer: {
            publicKey: '',
            privateKey: null,
          },
          snarker: this.snarker(),
        });

      case 'auto':
        // Check localStorage for URL parameter args
        const args = this.getWebnodeArgsFromStorage();
        if (args) {
          console.log(
            'WebNode: Using webnodeArgs from localStorage (URL parameter)',
          );
          return of({
            network: args.network || DEFAULT_NETWORK,
            blockProducer: args.blockProducer || {
              publicKey: '',
              privateKey: null,
            },
            snarker: this.snarker(args),
          });
        }

        // No configuration found - start in observer mode
        console.log(
          'WebNode: No configuration found - starting in observer mode',
        );
        return of({
          blockProducer: {
            publicKey: '',
            privateKey: null,
          },
          network: DEFAULT_NETWORK,
          snarker: this.snarker(),
        });
    }
  }

  /**
   * The web node's further options. The frontier store (I5e) keeps the
   * node's verified ledgers and blocks in this browser's Origin Private File
   * System, so that a reload restores from them instead of downloading them
   * again: on if the build says so, and `webnodeFrontierStore` in
   * localStorage (`on` or `off`) overrides the build.
   * `webnodeCheckpointSecs` sets the seconds between its checkpoints, for
   * tests.
   */
  private runOptions(): {
    frontierStore: boolean;
    checkpointIntervalSecs?: number;
  } {
    const storage = getLocalStorage();
    const chosen = storage?.getItem('webnodeFrontierStore');
    const frontierStore =
      chosen === 'on' ||
      (chosen !== 'off' && !!CONFIG.globalConfig?.webNodeFrontierStore);
    const checkpointSecs = Number(storage?.getItem('webnodeCheckpointSecs'));
    return {
      frontierStore,
      ...(checkpointSecs > 0 ? { checkpointIntervalSecs: checkpointSecs } : {}),
    };
  }

  private getWebnodeArgsFromStorage(): any | null {
    // localStorage value is set in ../../app.component.ts
    // from URL query param `a`.
    const raw = getLocalStorage()?.getItem('webnodeArgs');
    if (raw === null) {
      return null;
    }
    try {
      return JSON.parse(atob(raw));
    } catch (error) {
      console.error(
        'WebNode: Failed to parse webnodeArgs from localStorage:',
        error,
      );
      return null;
    }
  }

  startWasm$(config: WebNodeConfiguration): Observable<any> {
    if (isBrowser()) {
      return of(any(window).webnode).pipe(
        switchMap((wasm: any) => {
          this.wasm$.next(wasm);

          // nb: this is RUSTFLAGS "-Clink-args=--max-memory=4294967296" (4GiB)
          // nb: in .cargo/config.toml for the wasm32 target the div by 65536
          // nb: is because the WASM web API requires memory size to be specified
          // nb: in terms of 64KiB *pages*
          // todo: move to wherever angular injects `this.memory`
          this.memory.maximum = 4294967296 / 65536;
          // W1: a WASM on another host runs only once its bytes have the
          // SHA-256 this page pins; the glue then compiles those bytes. The
          // prover host (block_prover_host.js) reads the same pin.
          const pin = parsePinnedWasm(
            CONFIG.globalConfig?.webNodeWasmUrl,
            CONFIG.globalConfig?.webNodeWasmSha256,
          );
          if (pin) {
            any(globalThis).__MINA_WEBNODE_WASM__ = Object.freeze({ ...pin });
          }
          const bytes$ = pin ? from(fetchPinnedWasm(pin)) : of(undefined);
          return bytes$.pipe(
            switchMap(bytes =>
              from(wasm.default(bytes, new WebAssembly.Memory(this.memory))),
            ),
            map(() => wasm),
          );
        }),
        switchMap(wasm => {
          this.webnodeProgress$.next('Loaded');
          const urls = {
            // The node fetches these from a worker, so make relative ones
            // absolute: a build then serves from any host and port.
            seedUrls: (CONFIG.globalConfig.webNodeSeedUrls ?? []).map(url =>
              new URL(url, window.location.href).toString(),
            ),
            fixedSeeds: CONFIG.globalConfig.webNodeBootNodes,
          };
          const genesisUrl = CONFIG.globalConfig.webNodeGenesisUrl;
          let privateKey = config.blockProducer.privateKey;
          // P5.3: a node that produces blocks runs no SNARK worker. Its one
          // prover can't stop a SNARK proof for a block proof, which could
          // then miss its slot; `run` refuses both.
          const snarker = privateKey ? undefined : config.snarker;
          if (privateKey && config.snarker) {
            console.warn(
              'WebNode: SNARK worker off: this tab produces blocks. Sell ' +
                'SNARK work from another tab, without a block producer key.',
            );
          }
          console.log(
            'webnode config: has private key?',
            !!privateKey,
            'seed urls?',
            urls,
          );

          const options = this.runOptions();
          console.log('webnode config: run options', JSON.stringify(options));

          // W1: the STUN and TURN servers, from a file next to the page
          // when the build names one; the node's built-in STUN servers
          // otherwise. Their credentials stay out of the log.
          const iceServers$ = from(
            loadIceServers(
              CONFIG.globalConfig?.webNodeIceServersUrl,
              window.location.href,
            ),
          );
          return iceServers$.pipe(
            switchMap(iceServers =>
              from(
                wasm.run(
                  privateKey,
                  urls.seedUrls,
                  urls.fixedSeeds,
                  // The node fetches it from a worker, so make it absolute.
                  genesisUrl
                    ? new URL(genesisUrl, window.location.href).href
                    : null,
                  config.network,
                  null,
                  // P5.3: a SNARK worker for a public key, proving in the
                  // node's isolated prover.
                  snarker?.publicKey ?? null,
                  snarker?.fee ?? null,
                  iceServers ? { ...options, iceServers } : options,
                ),
              ),
            ),
          );
        }),
        tap((webnode: any) => {
          any(window).webnode = webnode;
          this.webnode$.next(webnode);
          this.webnodeProgress$.next('Started');
        }),
        catchError(error => {
          console.error('WebNode failed to start:', error.message);
          return throwError(() => new Error(error.message));
        }),
      );
    }
    return EMPTY;
  }

  get status$(): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode => from(any(webnode).status())),
    );
  }

  get verification$(): Observable<any> {
    return this.verificationQuery$();
  }

  /**
   * One answer, then complete. `hashes` looks receipts up by state hash; a
   * web node built before R26 ignores it and answers a page.
   */
  verificationQuery$(
    limit?: number,
    offset?: number,
    hashes?: string[],
  ): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      take(1),
      switchMap(webnode => {
        const stats = any(webnode).stats?.();
        if (typeof stats?.verification !== 'function') {
          throw Object.assign(new Error('Verification API not supported'), {
            name: 'VerificationUnsupportedError',
          });
        }
        return from(
          stats.verification(
            limit,
            offset,
            hashes?.length ? hashes : undefined,
          ),
        );
      }),
    );
  }

  get blockProducerStats$(): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode => from(any(webnode).stats().block_producer())),
    );
  }

  get peers$(): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode => from(any(webnode).state().peers())),
      tap((peers: any) => {
        if (
          !this.firstPeerConnected &&
          peers.some(
            (p: any) => p.connection_status === DashboardPeerStatus.CONNECTED,
          )
        ) {
          this.firstPeerConnected = true;
          this.webnodeProgress$.next('Connected');
        }
      }),
    );
  }

  get messageProgress$(): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode => from(any(webnode).state().message_progress())),
    );
  }

  /** `/stats/sync?limit=1`: the newest sync snapshot only. Without the limit
   * the node copies and serializes all 256 it keeps (tens of MB of JSON in
   * WASM memory) on every dashboard poll (WN-MEM). */
  get sync$(): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode => from(any(webnode).stats().sync(1))),
    );
  }

  get accounts$(): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode =>
        from(any(webnode).ledger().latest().accounts().all()),
      ),
    );
  }

  get bestChainUserCommands$(): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode =>
        from(any(webnode).transition_frontier().best_chain().user_commands()),
      ),
    );
  }

  sendPayment$(payment: any): Observable<any> {
    if (!CONFIG.controls?.send) {
      return throwError(
        () => new Error('Sending is disabled in this view-only build'),
      );
    }
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode =>
        from(any(webnode).transaction_pool().inject().payment(payment)),
      ),
    );
  }

  get transactionPool$(): Observable<any> {
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode => from(any(webnode).transaction_pool().get())),
    );
  }

  get envBuildDetails$(): Observable<any> {
    return this.wasm$.asObservable().pipe(
      filter(Boolean),
      map(webnode => webnode.build_env()),
    );
  }

  actions$(path: string): Observable<any> {
    let slot: string | number = path.split('?id=')[1];
    if (!isNaN(Number(slot))) {
      slot = Number(slot);
    }
    return this.webnode$.asObservable().pipe(
      filter(Boolean),
      switchMap(webnode => webnode.stats().actions(slot)),
    );
  }
}
