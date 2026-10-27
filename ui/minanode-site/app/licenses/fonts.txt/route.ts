// The SIL Open Font License notices for the self-hosted fonts, written to
// out/licenses/fonts.txt at build time from the design system's copies.
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

export const dynamic = "force-static";

const files = [
  ["Inter", "LICENSE-Inter.txt"],
  ["JetBrains Mono", "LICENSE-JetBrainsMono.txt"],
];

function fontsDir() {
  const candidates = [
    join(process.cwd(), "node_modules/@minanode/design-system/src/fonts"),
    join(process.cwd(), "../node_modules/@minanode/design-system/src/fonts"),
    join(process.cwd(), "../design-system/src/fonts"),
  ];
  const dir = candidates.find(existsSync);
  if (!dir) throw new Error("design-system fonts directory not found");
  return dir;
}

export async function GET() {
  const dir = fontsDir();
  const parts = await Promise.all(
    files.map(async ([name, file]) => {
      const text = await readFile(join(dir, file), "utf8");
      return `${name}\n${"=".repeat(name.length)}\n\n${text.trim()}\n`;
    }),
  );
  const body = [
    "Fonts used on minanode.org",
    "",
    "minanode.org serves Inter and JetBrains Mono (variable, Latin and Latin",
    "Extended subsets) from its own origin. Both are licensed under the SIL Open",
    "Font License, Version 1.1. Their notices and licence text follow.",
    "",
    ...parts,
  ].join("\n");
  return new Response(body, {
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}
