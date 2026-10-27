// Install the Chrome revision matched to the locked puppeteer-core dependency.
// No browser is started. stdout is the executable path for CHROME.
import { install, Browser } from '@puppeteer/browsers';
import { PUPPETEER_REVISIONS } from 'puppeteer-core';
import { resolve } from 'node:path';

const browser = await install({
  browser: Browser.CHROMEHEADLESSSHELL,
  buildId: PUPPETEER_REVISIONS['chrome-headless-shell'],
  cacheDir: resolve(process.argv[2] ?? '.browser'),
});
console.log(browser.executablePath);
