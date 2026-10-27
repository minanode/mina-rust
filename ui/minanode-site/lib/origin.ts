/** The site's origin. A preview build sets NEXT_PUBLIC_SITE_ORIGIN (such as a
 * pages.dev address), so its download commands fetch from the preview. */
export const siteOrigin =
  process.env.NEXT_PUBLIC_SITE_ORIGIN?.replace(/\/+$/, "") ??
  "https://minanode.org";

/** A preview build asks search engines not to index it. */
export const isPreview = siteOrigin !== "https://minanode.org";
