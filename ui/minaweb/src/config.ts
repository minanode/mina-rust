import type { BuildConfig } from "./boot/build-config";

import { localRelayConfig } from "./boot/local-relay";

declare const __MINAWEB_LOCAL__: boolean;
declare const __MINAWEB_CONFIG__: BuildConfig;

/** The web node this build starts (vite.config.ts compiles it in). */
export const CONFIG: BuildConfig = __MINAWEB_LOCAL__
  ? await localRelayConfig(__MINAWEB_CONFIG__)
  : __MINAWEB_CONFIG__;
