/**
 * One level of boxes on a page. A bordered surface (a Card, a KeyValueList)
 * is the box; a box-like pattern inside it (an empty state, an alert, an
 * inline confirmation) drops its own border there, and keeps at most a tint.
 * Tailwind reads these class names from this file.
 */

/** No border inside a surface. */
export const borderlessInSurface =
  "in-data-[slot=card]:border-0 in-data-[slot=key-value-list]:border-0";

/** No border, fill or padding inside a surface: the surface pads it. */
export const flatInSurface =
  "in-data-[slot=card]:border-0 in-data-[slot=card]:bg-transparent in-data-[slot=card]:p-0 in-data-[slot=key-value-list]:border-0 in-data-[slot=key-value-list]:bg-transparent in-data-[slot=key-value-list]:p-0";
