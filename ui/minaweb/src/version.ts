declare const __MINAWEB_VERSION__: string | null;

/** The release this page was built for (vite.config.ts reads it from the
 * workspace's Cargo.toml): the version the node's build_env reports too. */
export const VERSION: string | null =
  typeof __MINAWEB_VERSION__ === "string" ? __MINAWEB_VERSION__ : null;

/** The start and boot screens' footer: the version, as the node's pages
 * show it, and the build's commit. */
export function buildLabel(
  commit: string | null,
  version: string | null = VERSION,
): string | null {
  const build = commit ? `build ${commit.slice(0, 12)}` : null;
  return [version, build].filter(Boolean).join(", ") || null;
}
