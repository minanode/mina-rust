/// <reference types="vitest/config" />
import { existsSync, readFileSync, statSync, createReadStream } from "node:fs";
import { createRequire } from "node:module";
import { extname, join, normalize, resolve, sep } from "node:path";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

import { themeInitScript } from "@minanode/design-system/lib/theme";

import { readBuildConfig } from "./src/boot/build-config.ts";

/**
 * The web node this app starts is chosen when it is built, from MINAWEB_*
 * variables (tools/regression/webnode/build.sh sets them; see
 * src/boot/build-config.ts). For development, MINAWEB_ASSETS names the
 * browser directory of a built web node: the dev server serves its WASM,
 * genesis and seed list, and the config comes from its build-info.json.
 */
const assets = process.env.MINAWEB_ASSETS
  ? resolve(process.env.MINAWEB_ASSETS)
  : null;

function buildInfo(): Record<string, unknown> | null {
  if (!assets) return null;
  const file = join(assets, "build-info.json");
  if (!existsSync(file)) {
    throw new Error(`MINAWEB_ASSETS: no build-info.json in ${assets}`);
  }
  return JSON.parse(readFileSync(file, "utf8"));
}

/** The release this page is built for: the workspace's version in the
 * repository's Cargo.toml, the version the node's own build_env reports. */
function workspaceVersion(): string | null {
  const file = resolve(import.meta.dirname, "../../Cargo.toml");
  if (!existsSync(file)) return null;
  const section = readFileSync(file, "utf8").split(
    /^\[workspace\.package\]$/m,
  )[1];
  return (
    section?.split(/^\[/m)[0].match(/^version\s*=\s*"([^"]+)"/m)?.[1] ?? null
  );
}

// The unit tests compile no page, so they need no web node.
const config = process.env.VITEST
  ? null
  : readBuildConfig(process.env, buildInfo());

// The browser shares memory between the node's threads only on a page that
// is cross-origin isolated: these two headers, on every response.
const isolation = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

const types: Record<string, string> = {
  ".js": "text/javascript",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".txt": "text/plain",
  ".postcard": "application/octet-stream",
};

/** In development, the built web node's files under MINAWEB_ASSETS. */
function webNodeAssets(): Plugin {
  return {
    name: "minaweb-webnode-assets",
    configureServer(server) {
      if (!assets) return;
      server.middlewares.use((req, res, next) => {
        const path = decodeURIComponent((req.url ?? "").split("?")[0]);
        if (!/^\/(assets\/webnode\/|webnode-[a-z]+\/)/.test(path)) {
          return next();
        }
        const file = normalize(join(assets, path));
        if (!file.startsWith(assets + sep) || !existsSync(file)) return next();
        if (!statSync(file).isFile()) return next();
        res.setHeader(
          "Content-Type",
          types[extname(file)] ?? "application/octet-stream",
        );
        createReadStream(file).pipe(res);
      });
    },
  };
}

const fontLicences = [
  ["Inter", "LICENSE-Inter.txt"],
  ["JetBrains Mono", "LICENSE-JetBrainsMono.txt"],
] as const;

function fontsText(): string {
  const require = createRequire(import.meta.url);
  const parts = fontLicences.map(([name, file]) => {
    const path = require.resolve(`@minanode/design-system/fonts/${file}`);
    const text = readFileSync(path, "utf8").trim();
    return `${name}\n${"=".repeat(name.length)}\n\n${text}\n`;
  });
  return [
    "Fonts used by the web node page",
    "",
    "The page serves Inter and JetBrains Mono (variable, Latin and Latin",
    "Extended subsets) from its own origin. Both are licensed under the SIL",
    "Open Font License, Version 1.1. Their notices and licence text follow.",
    "",
    ...parts,
  ].join("\n");
}

/** The theme script, a file so that a Content-Security-Policy can stay
 * `script-src 'self'`, and the font licences. */
function pageFiles(): Plugin {
  const files: Record<string, () => string> = {
    "theme-init.js": () => themeInitScript,
    "licenses/fonts.txt": fontsText,
  };
  return {
    name: "minaweb-page-files",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const file = files[(req.url ?? "").replace(/^\//, "")];
        if (!file) return next();
        res.setHeader(
          "Content-Type",
          req.url?.endsWith(".js") ? "text/javascript" : "text/plain",
        );
        res.end(file());
      });
    },
    generateBundle(_options, bundle) {
      // Record the exact installed package behind every retained module,
      // including nested versions. Workspace code uses the root licence.
      const packages = new Map<
        string,
        { name: string; version: string; path: string }
      >();
      for (const output of Object.values(bundle)) {
        if (output.type !== "chunk") continue;
        for (const id of Object.keys(output.modules)) {
          const match = id
            .replaceAll("\\", "/")
            .match(/^(.*\/node_modules\/(?:@[^/]+\/)?[^/]+)\//);
          if (!match) continue;
          const root = match[1];
          const info = JSON.parse(
            readFileSync(join(root, "package.json"), "utf8"),
          );
          const base =
            resolve(import.meta.dirname, "..").replaceAll("\\", "/") + "/";
          if (!root.startsWith(base))
            throw new Error(`Package outside ui: ${info.name}`);
          const path = root.slice(base.length);
          packages.set(path, { name: info.name, version: info.version, path });
        }
      }
      this.emitFile({
        type: "asset",
        fileName: "licenses/bundle-packages.json",
        source:
          JSON.stringify(
            [...packages.values()].sort((a, b) =>
              a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
            ),
            null,
            2,
          ) + "\n",
      });
      for (const [fileName, source] of Object.entries(files)) {
        this.emitFile({ type: "asset", fileName, source: source() });
      }
    },
  };
}

export default defineConfig({
  plugins: [react(), tailwindcss(), webNodeAssets(), pageFiles()],
  define: {
    __MINAWEB_CONFIG__: JSON.stringify(config),
    __MINAWEB_LOCAL__: process.env.MINAWEB_LOCAL === "1",
    __MINAWEB_VERSION__: JSON.stringify(workspaceVersion()),
  },
  build: {
    outDir: "dist",
    assetsDir: "assets/app",
    target: "es2022",
    sourcemap: false,
    assetsInlineLimit: 0,
  },
  server: { headers: isolation },
  preview: { headers: isolation },
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    environment: "node",
  },
});
