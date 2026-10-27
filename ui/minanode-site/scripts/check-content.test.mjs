import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { createHash, generateKeyPairSync } from "node:crypto";
import { spawnSync } from "node:child_process";
import { join, resolve } from "node:path";

// A separate checkout-shaped fixture keeps all test trust material out of the site.
const site = resolve(new URL("..", import.meta.url).pathname);
const scratch = join(site, "../../target/test-data/release-signing");
mkdirSync(scratch, { recursive: true });
const { publicKey } = generateKeyPairSync("ed25519");
const publicBytes = Buffer.from(
  publicKey.export({ format: "jwk" }).x,
  "base64url",
);
const blob = Buffer.concat([
  Buffer.from("0000000b7373682d6564323535313900000020", "hex"),
  publicBytes,
]);
const realSigners = `release@minanode.org ssh-ed25519 ${blob.toString("base64")} release signing test\n`;
const fingerprint = `SHA256:${createHash("sha256").update(blob).digest("base64").replace(/=+$/, "")}`;
const pendingSigners = "release@minanode.org ssh-ed25519 RELEASE_KEY_PENDING\n";

const composeFiles = [
  "docker-compose.yml",
  "docker-compose.block-producer.yml",
];

function fixture(run) {
  const root = mkdtempSync(join(scratch, "content-test."));
  const target = join(root, "ui/minanode-site");
  const docs = join(root, "docs/release");
  for (const dir of [
    join(target, "scripts"),
    join(target, "content"),
    join(target, "out"),
    docs,
  ])
    mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(target, "scripts/check-content.mjs"),
    readFileSync(join(site, "scripts/check-content.mjs")),
  );
  const setup = (
    signers,
    value,
    html = "<p>Verify <code>SHA256SUMS</code> first.</p>",
  ) => {
    writeFileSync(join(docs, "allowed_signers"), signers);
    writeFileSync(join(target, "out/allowed_signers"), signers);
    writeFileSync(
      join(target, "content/release.ts"),
      `export const releaseSigningFingerprint = "${value}";`,
    );
    writeFileSync(join(target, "out/index.html"), html);
    // The Docker page's compose files, served as the repository has them.
    mkdirSync(join(target, "out/docker"), { recursive: true });
    for (const file of composeFiles) {
      writeFileSync(join(root, file), `# ${file}\n`);
      writeFileSync(join(target, "out/docker", file), `# ${file}\n`);
    }
  };
  const check = final =>
    spawnSync(process.execPath, [join(target, "scripts/check-content.mjs")], {
      cwd: target,
      env: { ...process.env, RELEASE_FINAL: final ? "1" : "0" },
      encoding: "utf8",
    });
  try {
    run({ setup, check, target });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("pending signing markers are allowed in a preview and rejected for release", () =>
  fixture(({ setup, check }) => {
    setup(
      pendingSigners,
      "SIGNING_FINGERPRINT_PENDING",
      "<p>SIGNING_FINGERPRINT_PENDING</p>",
    );
    assert.equal(check(false).status, 0);
    assert.notEqual(check(true).status, 0);
  }));
test("a matching real public key and fingerprint pass the final gate", () =>
  fixture(({ setup, check }) => {
    setup(realSigners, fingerprint);
    assert.equal(check(true).status, 0);
  }));
test("a stale fingerprint fails", () =>
  fixture(({ setup, check }) => {
    setup(realSigners, "SHA256:wrong");
    assert.match(check(true).stderr, /does not match/);
  }));
test("a stale served signer file fails", () =>
  fixture(({ setup, check, target }) => {
    setup(realSigners, fingerprint);
    writeFileSync(join(target, "out/allowed_signers"), pendingSigners);
    assert.match(check(false).stderr, /differs from/);
  }));
test("a pending fingerprint left in rendered output fails", () =>
  fixture(({ setup, check }) => {
    setup(realSigners, fingerprint, "<p>SIGNING_FINGERPRINT_PENDING</p>");
    assert.notEqual(check(true).status, 0);
  }));
test("unfinished download URLs still fail in preview mode", () =>
  fixture(({ setup, check }) => {
    setup(
      pendingSigners,
      "SIGNING_FINGERPRINT_PENDING",
      "<p>https://download.invalid/file</p>",
    );
    assert.notEqual(check(false).status, 0);
  }));
test("inline collisions still fail", () =>
  fixture(({ setup, check }) => {
    setup(
      pendingSigners,
      "SIGNING_FINGERPRINT_PENDING",
      "<p><code>flag</code>reads</p>",
    );
    assert.notEqual(check(false).status, 0);
  }));

test("public developer showcase exports fail", () =>
  fixture(({ setup, check, target }) => {
    setup(realSigners, fingerprint);
    mkdirSync(join(target, "out/design-system"));
    writeFileSync(
      join(target, "out/design-system/index.html"),
      "<h1>Specimens</h1>",
    );
    assert.match(check(false).stderr, /must not be exported/);
  }));
test("public developer showcase links fail", () =>
  fixture(({ setup, check }) => {
    for (const href of [
      "/design-system/",
      "https://minanode.org/design-system/?theme=dark",
    ]) {
      setup(realSigners, fingerprint, `<a href="${href}">Specimens</a>`);
      assert.match(check(false).stderr, /link to the developer showcase/);
    }
  }));
test("public developer showcase sitemap entries fail", () =>
  fixture(({ setup, check, target }) => {
    setup(realSigners, fingerprint);
    writeFileSync(
      join(target, "out/sitemap.xml"),
      "<urlset><url><loc>https://minanode.org/design-system/</loc></url></urlset>",
    );
    assert.match(check(false).stderr, /sitemap links/);
  }));

test("a served compose file that differs from the repository's fails", () =>
  fixture(({ setup, check, target }) => {
    setup(pendingSigners, "SIGNING_FINGERPRINT_PENDING");
    assert.equal(check(false).status, 0);
    writeFileSync(join(target, "out/docker/docker-compose.yml"), "# old\n");
    const result = check(false);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /docker\/docker-compose\.yml differs/);
  }));
