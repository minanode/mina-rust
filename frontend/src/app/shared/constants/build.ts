import type { FrontendControls } from '@shared/types/core/environment/mina-env.type';

/**
 * Stamped in by `ng build --define`, as tools/regression/webnode/build.sh
 * does. `typeof` keeps a build without the define working: esbuild replaces
 * the identifier, and without it the global is simply undefined.
 */
declare const MINA_FRONTEND_COMMIT: string;
declare const MINA_FRONTEND_CONTROLS: string;

/** The commit this frontend was built from, or '' if not stamped. */
export const BUILD_FRONTEND_COMMIT: string =
  typeof MINA_FRONTEND_COMMIT === 'string' ? MINA_FRONTEND_COMMIT.trim() : '';

export const VIEW_ONLY: FrontendControls = {
  producer: false,
  send: false,
  snarker: false,
};
export const ALL_CONTROLS: FrontendControls = {
  producer: true,
  send: true,
  snarker: true,
};

/**
 * MINA_FRONTEND_CONTROLS: empty for a view-only build, or a comma-separated
 * list of `producer`, `send` and `snarker`, or `all`. Unknown words are
 * ignored.
 */
export function parseControls(value: string | undefined): FrontendControls {
  const words = (value ?? '')
    .split(',')
    .map(word => word.trim().toLowerCase())
    .filter(Boolean);
  if (words.includes('all')) return { ...ALL_CONTROLS };
  return {
    producer: words.includes('producer'),
    send: words.includes('send'),
    snarker: words.includes('snarker'),
  };
}

/** For the showcase environments (native dashboard, web node). */
export const BUILD_FRONTEND_CONTROLS: FrontendControls = parseControls(
  typeof MINA_FRONTEND_CONTROLS === 'string' ? MINA_FRONTEND_CONTROLS : '',
);
