import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

for (const missing of ["isolation", "integrity"]) {
  test(`missing ${missing} failure screen exits nonzero with evidence and cleanup`, () => {
    const root = mkdtempSync(join(tmpdir(), "minaweb-harness-"));
    try {
      const stub = join(root, "node_modules/puppeteer-core");
      mkdirSync(stub, { recursive: true });
      writeFileSync(join(root, "package.json"), "{}");
      writeFileSync(join(stub, "index.js"), `
        const fs = require('node:fs');
        let contextNumber = 0;
        module.exports.launch = async () => ({
          createBrowserContext: async () => {
            const number = ++contextNumber;
            return {
              close: async () => fs.writeFileSync(process.env.OUT + '/context-' + number + '-closed', ''),
              newPage: async () => ({
                setViewport: async () => {}, on: () => {}, setRequestInterception: async () => {},
                goto: async () => {},
                evaluate: async fn => fn.toString().includes('dataset.boot')
                  ? (process.env.MISSING === 'integrity' && number === 1 ? 'failed:isolation' : 'start') : 'failure text',
                screenshot: async ({ path }) => fs.writeFileSync(path, 'screenshot fixture'),
              }),
            };
          },
          close: async () => fs.writeFileSync(process.env.OUT + '/browser-closed', ''),
        });
      `);
      const preload = join(root, "clock.mjs");
      writeFileSync(preload, `let now = Date.now(); Date.now = () => now += 121000;
        const timer = globalThis.setTimeout; globalThis.setTimeout = fn => timer(fn, 0);`);
      const out = join(root, "out");
      const result = spawnSync(process.execPath, ["--import", preload, new URL("./minaweb-check.mjs", import.meta.url).pathname], {
        env: { ...process.env, OUT: out, BASE: "https://example.org", CHROME: "fixture", PROFILE_DIR: join(root, "profile"), PUPPETEER_DIR: root, MISSING: missing },
        encoding: "utf8", timeout: 15000,
      });
      assert.equal(result.status, 1, result.stderr);
      const summary = JSON.parse(readFileSync(join(out, "summary.json")));
      assert.equal(summary.ok, false);
      assert.match(summary.fatal, /failure-screen checks failed/);
      assert.equal(summary.failures[missing === "isolation" ? "not-isolated" : "integrity"].ok, false);
      assert.ok(existsSync(join(out, "boot-not-isolated.png")));
      assert.ok(existsSync(join(out, "boot-integrity.png")));
      for (const name of ["context-1-closed", "context-2-closed", "browser-closed"]) assert.ok(existsSync(join(out, name)));
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
}
