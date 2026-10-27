import { releaseLabel } from "@/content/release";
// Open Graph and touch icons, rendered to PNG at build time with next/og.
// Fonts are read from the pinned @fontsource/inter package on disk; nothing is
// fetched over the network.
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { ImageResponse } from "next/og";

// The twin-arches mark (ui/design-system/src/patterns/logo.tsx), drawn on a
// 48 x 48 grid. With a background it becomes an opaque tile for the touch
// icon, which iOS rounds itself.
const ARCHES = "M6 36V22C6 9 24 9 24 22V36M24 22C24 9 42 9 42 22V36";

const markSvg = (background?: string) =>
  background
    ? `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect width="48" height="48" fill="${background}"/><g transform="translate(9.6,9.6) scale(0.6)"><path d="${ARCHES}" fill="none" stroke="#12A594" stroke-width="5.5" stroke-linecap="square"/></g></svg>`
    : `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><path d="${ARCHES}" fill="none" stroke="#12A594" stroke-width="5.5" stroke-linecap="square"/></svg>`;

const markDataUri = (background?: string) =>
  `data:image/svg+xml;base64,${Buffer.from(markSvg(background)).toString("base64")}`;

async function interFont(weight: 400 | 600 | 700) {
  const file = `inter-latin-${weight}-normal.woff`;
  const candidates = [
    join(process.cwd(), "node_modules/@fontsource/inter/files", file),
    join(process.cwd(), "../node_modules/@fontsource/inter/files", file),
  ];
  const path = candidates.find(existsSync);
  if (!path) throw new Error(`Inter ${weight} not found; run npm install`);
  return readFile(path);
}

export const ogAlt =
  "minanode logo with the words: Client diversity for Mina. A native node in Rust. A full node in the browser.";
export const ogSize = { width: 1200, height: 630 };

export async function renderOgImage() {
  const [regular, semibold, bold] = await Promise.all([
    interFont(400),
    interFont(600),
    interFont(700),
  ]);
  return new ImageResponse(
    (
      <div
        style={{
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
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 22 }}>
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={markDataUri()} width={64} height={64} alt="" />
          <div style={{ display: "flex", fontSize: 44, letterSpacing: -2 }}>
            <span style={{ fontWeight: 600 }}>mina</span>
            <span style={{ fontWeight: 400, color: "#a1afb4" }}>node</span>
          </div>
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          <div
            style={{
              fontSize: 76,
              fontWeight: 700,
              letterSpacing: -3,
              lineHeight: 1.05,
            }}
          >
            Client diversity for Mina.
          </div>
          <div
            style={{
              fontSize: 54,
              fontWeight: 700,
              letterSpacing: -2,
              lineHeight: 1.1,
              color: "#2dbba6",
            }}
          >
            A native node in Rust. A full node in the browser.
          </div>
        </div>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 16,
            fontSize: 26,
            color: "#a3b1bb",
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 12,
              padding: "10px 20px",
              borderRadius: 999,
              border: "2px solid rgba(95,211,183,0.45)",
              color: "#5fd3b7",
            }}
          >
            <div
              style={{
                width: 12,
                height: 12,
                borderRadius: 999,
                backgroundColor: "#5fd3b7",
              }}
            />
            {releaseLabel}, available now
          </div>
          <div>A community-led second Mina client</div>
        </div>
      </div>
    ),
    {
      ...ogSize,
      fonts: [
        { name: "Inter", data: regular, weight: 400, style: "normal" },
        { name: "Inter", data: semibold, weight: 600, style: "normal" },
        { name: "Inter", data: bold, weight: 700, style: "normal" },
      ],
    },
  );
}

export function renderAppleIcon() {
  return new ImageResponse(
    (
      // eslint-disable-next-line @next/next/no-img-element
      <img src={markDataUri("#0a0f14")} width={180} height={180} alt="" />
    ),
    { width: 180, height: 180 },
  );
}
