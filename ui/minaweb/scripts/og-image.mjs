// Renders public/og.png, the card that chat apps and social sites show for a
// minaweb.org link. It follows the minanode.org card
// (ui/minanode-site/lib/og.tsx) and carries no version, so it does not go
// stale between releases. Run it from ui/ after changing the card, and commit
// the PNG:
//
//   node minaweb/scripts/og-image.mjs
//
// next/og comes from the minanode-site workspace, Inter from the pinned
// @fontsource/inter package on disk; nothing is fetched over the network.
import { readFile, writeFile } from "node:fs/promises";

import { ImageResponse } from "next/og.js";
import { createElement as h } from "react";

const out = new URL("../public/og.png", import.meta.url);

// The twin-arches mark (ui/design-system/src/patterns/logo.tsx), drawn on a
// 48 x 48 grid.
const ARCHES = "M6 36V22C6 9 24 9 24 22V36M24 22C24 9 42 9 42 22V36";
const mark = `data:image/svg+xml;base64,${Buffer.from(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><path d="${ARCHES}" fill="none" stroke="#12A594" stroke-width="5.5" stroke-linecap="square"/></svg>`,
).toString("base64")}`;

const inter = weight =>
  readFile(
    new URL(
      `../../node_modules/@fontsource/inter/files/inter-latin-${weight}-normal.woff`,
      import.meta.url,
    ),
  );
const [regular, semibold, bold] = await Promise.all([
  inter(400),
  inter(600),
  inter(700),
]);

const card = h(
  "div",
  {
    style: {
      width: "100%",
      height: "100%",
      display: "flex",
      flexDirection: "column",
      justifyContent: "space-between",
      padding: "72px 80px",
      backgroundColor: "#0a0f14",
      backgroundImage:
        "radial-gradient(ellipse 70% 60% at 85% 0%, rgba(18,165,148,0.28), rgba(18,165,148,0) 70%)",
      color: "#f5f8f9",
      fontFamily: "Inter",
    },
  },
  h(
    "div",
    { style: { display: "flex", alignItems: "center", gap: 22 } },
    h("img", { src: mark, width: 64, height: 64, alt: "" }),
    h(
      "div",
      { style: { display: "flex", fontSize: 44, letterSpacing: -2 } },
      h("span", { style: { fontWeight: 600 } }, "mina"),
      h("span", { style: { fontWeight: 400, color: "#a1afb4" } }, "node"),
      h(
        "span",
        {
          style: {
            fontWeight: 400,
            color: "#a1afb4",
            marginLeft: 18,
            paddingLeft: 18,
            borderLeft: "2px solid #2a3740",
          },
        },
        "web node",
      ),
    ),
  ),
  h(
    "div",
    { style: { display: "flex", flexDirection: "column", gap: 12 } },
    h(
      "div",
      {
        style: {
          fontSize: 76,
          fontWeight: 700,
          letterSpacing: -3,
          lineHeight: 1.05,
        },
      },
      "A Mina node in your browser.",
    ),
    h(
      "div",
      {
        style: {
          fontSize: 54,
          fontWeight: 700,
          letterSpacing: -2,
          lineHeight: 1.1,
          color: "#2dbba6",
        },
      },
      "It checks every block itself.",
    ),
  ),
  h(
    "div",
    {
      style: {
        display: "flex",
        alignItems: "center",
        gap: 16,
        fontSize: 26,
        color: "#a3b1bb",
      },
    },
    h(
      "div",
      {
        style: {
          display: "flex",
          alignItems: "center",
          gap: 12,
          padding: "10px 20px",
          borderRadius: 999,
          border: "2px solid rgba(95,211,183,0.45)",
          color: "#5fd3b7",
        },
      },
      h("div", {
        style: {
          width: 12,
          height: 12,
          borderRadius: 999,
          backgroundColor: "#5fd3b7",
        },
      }),
      "minaweb.org",
    ),
    h("div", null, "Devnet. Nothing to install."),
  ),
);

const image = new ImageResponse(card, {
  width: 1200,
  height: 630,
  fonts: [
    { name: "Inter", data: regular, weight: 400, style: "normal" },
    { name: "Inter", data: semibold, weight: 600, style: "normal" },
    { name: "Inter", data: bold, weight: 700, style: "normal" },
  ],
});
await writeFile(out, Buffer.from(await image.arrayBuffer()));
console.log(`wrote ${out.pathname}`);
