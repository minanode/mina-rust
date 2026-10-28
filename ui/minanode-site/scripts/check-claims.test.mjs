import { test } from "node:test";
import assert from "node:assert/strict";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const site = resolve(new URL("..", import.meta.url).pathname);

function check(sources, extraClaims = "") {
  const scratch = mkdtempSync(join(site, ".claims-test-"));
  try {
    mkdirSync(join(scratch, "scripts"));
    mkdirSync(join(scratch, "out"));
    copyFileSync(join(site, "scripts/check-claims.mjs"), join(scratch, "scripts/check-claims.mjs"));
    writeFileSync(join(scratch, "CLAIMS.md"), `### C01 Test claim\n\n- Sources:${sources}\n- Figures: \`123\`\n- Caveat kept: private measurement.\n${extraClaims}`);
    writeFileSync(join(scratch, "out/index.html"), '<p data-claim="C01">123</p>');
    return spawnSync(process.execPath, [join(scratch, "scripts/check-claims.mjs")], { encoding: "utf8" });
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

test("coordinator sources survive Prettier wrapping after the field label", () => {
  for (const sources of [" `coordinator: private pre-release measurement`", "\n  `coordinator: private pre-release measurement`"]) {
    const result = check(sources);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /private measurements are not independently validated/);
  }
});

test("an empty source remains a failing claim", () => {
  const result = check("\n  ");
  assert.equal(result.status, 1);
  assert.match(result.stdout, /C01: no sources/);
});

test("an unknown wrapped source is not accepted as coordinator evidence", () => {
  const result = check("\n  `unknown-source`");
  assert.equal(result.status, 1);
  assert.match(result.stdout, /cannot read source reference unknown-source/);
});

test("an ID with two entries fails", () => {
  const result = check(" `coordinator: private pre-release measurement`", "\n### C01 Another claim\n\n- Sources: `coordinator: other`\n- Figures: none\n");
  assert.equal(result.status, 1);
  assert.match(result.stdout, /C01: more than one entry in CLAIMS.md/);
});
