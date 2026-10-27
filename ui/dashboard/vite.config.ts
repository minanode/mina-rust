/// <reference types="vitest/config" />
import { readFileSync } from "node:fs";
import type { IncomingMessage, ServerResponse } from "node:http";
import { createRequire } from "node:module";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

import { themeInitScript } from "@minanode/design-system/lib/theme";

// The node this dev server talks to: `mina node` serves the built dashboard
// itself, so in development Vite stands in for it and forwards the API.
const node = process.env.MINA_NODE ?? "http://127.0.0.1:3700";
// For a node that must only be read (a shared one): refuse every request but
// GET, HEAD and GraphQL queries before it leaves this machine.
const readOnly = process.env.MINA_NODE_READONLY === "1";

// The node's API routes the dashboard calls, matched whole: a prefix such as
// "/graphql" would also take the dashboard's own "/graphql-endpoint".
const api = [
  "^/status$",
  "^/build_env$",
  "^/stats/",
  "^/snarker/",
  "^/state(/|$)",
  "^/transaction-pool$",
  "^/snark-pool/",
  "^/graphql$",
  // The bug report; the page is "/report-a-problem".
  "^/report(/crash)?$",
];
const isApi = (url: string | undefined) => {
  const path = (url ?? "").split("?")[0];
  return api.some(pattern => new RegExp(pattern).test(path));
};

/** A request whose body the read-only check has read, kept for the proxy. */
type BufferedRequest = IncomingMessage & { rawBody?: Buffer };

const maxGraphqlBody = 64 * 1024;

/** True if a GraphQL document can only read. A mutation or subscription must
 * name its keyword (the `{ ... }` shorthand is a query), so a document without
 * either word can't write. A field or string with that word is refused too. */
const readsOnly = (query: unknown) =>
  typeof query === "string" && !/\b(mutation|subscription)\b/.test(query);

function refuse(res: ServerResponse) {
  res.statusCode = 405;
  res.setHeader("Content-Type", "application/json");
  res.end(
    JSON.stringify({
      error:
        "read-only dev server: only GET and GraphQL queries reach this node",
    }),
  );
}

/** Lets a GraphQL request through only if it can't write: a GET whose query
 * parameters read only, or a POST of one JSON operation that reads only. */
function readOnlyGraphql(
  req: BufferedRequest,
  res: ServerResponse,
  next: () => void,
) {
  if (req.method === "GET" || req.method === "HEAD") {
    const url = new URL(req.url ?? "/", "http://dev");
    if (url.searchParams.getAll("query").every(readsOnly)) next();
    else refuse(res);
    return;
  }
  if (req.method !== "POST") {
    refuse(res);
    return;
  }
  const chunks: Buffer[] = [];
  let size = 0;
  req.on("data", (chunk: Buffer) => {
    size += chunk.length;
    if (size <= maxGraphqlBody) chunks.push(chunk);
  });
  req.on("end", () => {
    const body = Buffer.concat(chunks);
    let request: unknown;
    try {
      request = JSON.parse(body.toString("utf8"));
    } catch {
      request = undefined;
    }
    const isQuery =
      typeof request === "object" &&
      request !== null &&
      !Array.isArray(request) &&
      readsOnly((request as { query?: unknown }).query);
    if (size > maxGraphqlBody || !isQuery) {
      refuse(res);
      return;
    }
    req.rawBody = body;
    next();
  });
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
    "Fonts used by the node dashboard",
    "",
    "The dashboard serves Inter and JetBrains Mono (variable, Latin and Latin",
    "Extended subsets) from the node itself. Both are licensed under the SIL",
    "Open Font License, Version 1.1. Their notices and licence text follow.",
    "",
    ...parts,
  ].join("\n");
}

/** Files the node serves next to the bundle: the theme script, which runs
 * before the first paint and is a file rather than inline so that the node's
 * Content-Security-Policy can stay `script-src 'self'`, and the font
 * licences. */
function nodeFiles(): Plugin {
  const files: Record<string, () => string> = {
    "theme-init.js": () => themeInitScript,
    "licenses/fonts.txt": fontsText,
  };
  return {
    name: "minanode-node-files",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const file = files[(req.url ?? "").replace(/^\//, "")];
        if (file) {
          res.setHeader(
            "Content-Type",
            req.url?.endsWith(".js") ? "text/javascript" : "text/plain",
          );
          res.end(file());
          return;
        }
        if (!readOnly || !isApi(req.url)) {
          next();
        } else if ((req.url ?? "").split("?")[0] === "/graphql") {
          readOnlyGraphql(req, res, next);
        } else if (req.method === "GET" || req.method === "HEAD") {
          next();
        } else {
          refuse(res);
        }
      });
    },
    generateBundle(_options, bundle) {
      // Rollup's retained modules are the packages shipped in the JS/CSS,
      // unlike the much wider npm lockfile (which includes build tools).
      const packages = new Set<string>();
      for (const output of Object.values(bundle)) {
        if (output.type !== "chunk") continue;
        for (const id of Object.keys(output.modules)) {
          const path = id.replaceAll("\\", "/");
          const match = path.match(/\/node_modules\/((?:@[^/]+\/)?[^/]+)/);
          if (match) packages.add(match[1]);
        }
      }
      this.emitFile({
        type: "asset",
        fileName: "licenses/bundle-packages.json",
        source: JSON.stringify([...packages].sort(), null, 2) + "\n",
      });
      for (const [fileName, source] of Object.entries(files)) {
        this.emitFile({ type: "asset", fileName, source: source() });
      }
    },
  };
}

export default defineConfig({
  plugins: [react(), tailwindcss(), nodeFiles()],
  build: {
    outDir: "dist",
    assetsDir: "assets",
    target: "es2022",
    sourcemap: false,
    // Fonts stay files: the node serves them with a long cache lifetime.
    assetsInlineLimit: 0,
  },
  server: {
    proxy: Object.fromEntries(
      api.map(path => [
        path,
        {
          target: node,
          changeOrigin: true,
          // The read-only check read a GraphQL body; send it on.
          configure: proxy => {
            proxy.on("proxyReq", (proxyReq, req) => {
              const body = (req as BufferedRequest).rawBody;
              if (body) {
                proxyReq.setHeader("Content-Length", body.length);
                proxyReq.write(body);
              }
            });
          },
        },
      ]),
    ),
  },
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    environment: "node",
  },
});
