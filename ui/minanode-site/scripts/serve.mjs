// Serve the static export for local previews: node scripts/serve.mjs DIR PORT
// Binds to 127.0.0.1 only. Directory paths serve their index.html, as a static
// host would; unknown paths get out/404.html with status 404.
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize, resolve, sep } from "node:path";

const root = resolve(process.argv[2] ?? "out");
const port = Number(process.argv[3] ?? 4270);
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".txt": "text/plain; charset=utf-8",
  ".xml": "application/xml",
};

async function isFile(path) {
  try {
    return (await stat(path)).isFile();
  } catch {
    return false;
  }
}

createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const path = normalize(decodeURIComponent(url.pathname));
  let file = join(root, path);
  if (file !== root && !file.startsWith(root + sep)) {
    res.writeHead(403).end();
    return;
  }
  if (!(await isFile(file))) file = join(file, "index.html");
  let status = 200;
  if (!(await isFile(file))) {
    file = join(root, "404.html");
    status = 404;
  }
  res.writeHead(status, {
    "content-type": types[extname(file)] ?? "application/octet-stream",
    "cache-control": "no-store",
  });
  res.end(await readFile(file));
}).listen(port, "127.0.0.1", () => {
  console.log(`serving ${root} on http://127.0.0.1:${port}`);
});
