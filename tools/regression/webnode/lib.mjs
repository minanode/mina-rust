// Helpers shared by the web node browser scripts. See ../README.md.
import { createRequire } from "node:module";
import { execFileSync, spawnSync } from "node:child_process";
import {
  closeSync,
  existsSync,
  ftruncateSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  statSync,
  writeSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const repoRoot = resolve(here, "../../..");

// puppeteer-core is not a dependency of the repo. Look for it in PUPPETEER_DIR,
// then next to these scripts, then in the web verifier's browser test, which
// depends on it (`npm ci` there installs it). Returns the module and the
// directory it was found from.
export function findPuppeteer() {
  const dirs = [process.env.PUPPETEER_DIR, here, join(repoRoot, "crates/web-verifier/tests/browser")].filter(Boolean);
  for (const dir of dirs) {
    try {
      return { puppeteer: createRequire(join(dir, "package.json"))("puppeteer-core"), dir };
    } catch {}
  }
  throw new Error(`puppeteer-core not found from ${dirs.join(", ")}: set PUPPETEER_DIR to a directory whose node_modules has it`);
}

export const loadPuppeteer = () => findPuppeteer().puppeteer;

// Locks on the shared machine's browser and node slots, the same ones
// ../lock.sh takes; see ../README.md, "Taking turns". Each is a flock(1) lock on
// <lock dir>/<name>.lock. The lock directory is MINA_LOCK_DIR, or else the main
// checkout's .work/locks, which all its worktrees share.
export function lockDir() {
  if (process.env.MINA_LOCK_DIR) return resolve(process.env.MINA_LOCK_DIR);
  let common;
  try {
    common = execFileSync("git", ["-C", here, "rev-parse", "--path-format=absolute", "--git-common-dir"], {
      stdio: ["ignore", "pipe", "ignore"],
    })
      .toString()
      .trim();
  } catch {
    throw new Error("not in a git checkout: set MINA_LOCK_DIR to the directory for the lock files");
  }
  return join(dirname(common), ".work", "locks");
}

// Whether this process already has the lock file open, as when it was started
// by `lock.sh browser`, whose descriptor it inherited with the lock on it.
function inherited(path) {
  try {
    return readdirSync("/proc/self/fd").some((fd) => {
      try {
        return readlinkSync(`/proc/self/fd/${fd}`) === path;
      } catch {
        return false;
      }
    });
  } catch {
    return false;
  }
}

const holderOf = (path) => {
  try {
    return readFileSync(path, "utf8").trim() || "(no holder recorded)";
  } catch {
    return "(no holder recorded)";
  }
};

// Take the first free lock among `names`, waiting while all are taken: up to
// `wait` or LOCK_WAIT seconds if set (0: don't wait), else for as long as it
// takes, trying again every LOCK_POLL seconds (15). `what` is recorded in the
// lock file for whoever waits. The lock is held until `release` or until this
// process exits. Returns null with MINA_NO_LOCK=1. Throws with `code` 75 when
// it gives up.
export async function takeLock(names, what, { log = console.error, wait: waitSeconds } = {}) {
  if (process.env.MINA_NO_LOCK === "1") {
    log(`MINA_NO_LOCK=1: not taking the ${names.join(" or ")} lock`);
    return null;
  }
  mkdirSync(lockDir(), { recursive: true });
  const dir = realpathSync(lockDir());
  for (const name of names) {
    const path = join(dir, `${name}.lock`);
    if (inherited(path)) {
      log(`holding ${path}, inherited from the process that started this one`);
      return { name, path, fd: null };
    }
  }
  const wait = waitSeconds ?? (process.env.LOCK_WAIT ? Number(process.env.LOCK_WAIT) : Infinity);
  if (Number.isNaN(wait)) throw new Error(`LOCK_WAIT=${process.env.LOCK_WAIT} is not a number of seconds`);
  const poll = Number(process.env.LOCK_POLL || 15);
  if (!(poll > 0)) throw new Error(`LOCK_POLL=${process.env.LOCK_POLL} is not a number of seconds`);
  const started = Date.now();
  let shown = null;
  for (;;) {
    for (const name of names) {
      const path = join(dir, `${name}.lock`);
      const fd = openSync(path, "a+");
      // flock(1) locks the open file that it shares with this process as its
      // descriptor 3; the lock stays on it after flock exits. Node opens files
      // close-on-exec, so the browser it starts doesn't inherit it.
      const result = spawnSync("flock", ["-n", "-x", "3"], { stdio: ["ignore", "ignore", "pipe", fd] });
      if (result.error) {
        closeSync(fd);
        throw new Error(`flock: ${result.error.message}. Install util-linux, or set MINA_NO_LOCK=1 on a machine you have to yourself`);
      }
      // 1 is "taken"; anything else is flock failing.
      if (result.status !== 0 && result.status !== 1) {
        closeSync(fd);
        throw new Error(`flock on ${path} failed (${result.status ?? result.signal}): ${result.stderr.toString().trim()}`);
      }
      if (result.status === 0) {
        ftruncateSync(fd, 0);
        const since = new Date().toISOString().slice(0, 19);
        writeSync(fd, `pid ${process.pid} since ${since}Z in ${process.cwd()}: ${what}\n`);
        log(`took ${path}`);
        return { name, path, fd };
      }
      closeSync(fd);
    }
    const holders = names.map((name) => `  ${name}: ${holderOf(join(dir, `${name}.lock`))}`).join("\n");
    if ((Date.now() - started) / 1000 >= wait) {
      const error = new Error(`gave up on ${names.join(" or ")} after ${wait} s; taken by:\n${holders}`);
      error.code = 75;
      error.holders = holders;
      throw error;
    }
    if (holders !== shown) {
      log(`waiting for ${names.join(" or ")} in ${dir} (LOCK_WAIT=${process.env.LOCK_WAIT ?? "unset"}); taken by:\n${holders}`);
      shown = holders;
    }
    await new Promise((r) => setTimeout(r, poll * 1000));
  }
}

export function release(lock) {
  if (lock?.fd != null) closeSync(lock.fd);
}

// The output directory: OUT, or the current directory.
export const outDir = () => resolve(process.env.OUT ?? ".");

// A file run.mjs writes with its browser's DevTools endpoint.
export const ENDPOINT_FILE = "browser.ws";

// The DevTools endpoint to attach to, from, in order:
// - WS, a ws:// endpoint;
// - RUN_DIR, the output directory of a run.mjs, which wrote browser.ws there;
// - the newest puppeteer profile in the temp directory. That is the newest
//   browser anyone started on this machine, which may not be yours.
export function browserEndpoint() {
  if (process.env.WS) return process.env.WS;
  if (process.env.RUN_DIR) return readFileSync(join(process.env.RUN_DIR, ENDPOINT_FILE), "utf8").trim();
  const tmp = tmpdir();
  const newest = readdirSync(tmp)
    .filter((d) => d.startsWith("puppeteer_dev_chrome_profile-"))
    .map((d) => join(tmp, d))
    .filter((d) => existsSync(join(d, "DevToolsActivePort")))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!newest) throw new Error("no running browser found: set RUN_DIR or WS");
  console.error(`attaching to the newest browser, ${newest}; set RUN_DIR to pick yours`);
  const [port, path] = readFileSync(join(newest, "DevToolsActivePort"), "utf8").trim().split("\n");
  return `ws://127.0.0.1:${port}${path}`;
}

export async function attach(puppeteer, protocolTimeout = 60000) {
  return puppeteer.connect({ browserWSEndpoint: browserEndpoint(), protocolTimeout });
}

// The web node's page: the first whose URL contains PAGE_MATCH, or else the
// first http(s) page.
export async function webnodePage(browser) {
  const match = process.env.PAGE_MATCH;
  const pages = await browser.pages();
  const page = pages.find((p) => (match ? p.url().includes(match) : /^https?:/.test(p.url())));
  if (!page) throw new Error(`no page matching ${match ?? "http"} among ${pages.map((p) => p.url()).join(", ")}`);
  return page;
}

// Seconds since `started`, and the UTC time of day, for log lines.
export const stamp = (started) => `${Math.round((Date.now() - started) / 1000)}s ${new Date().toISOString().slice(11, 19)}`;
