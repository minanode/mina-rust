import assert from 'node:assert/strict';
import { test } from 'node:test';
import { iOSversion } from '../src/app/shared/helpers/webnode.helper.ts';

// User agents as Safari and Chrome send them on iOS, and on desktops.
const iPhone17 =
  'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1';
const iPad17 =
  'Mozilla/5.0 (iPad; CPU OS 17_4_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4.1 Mobile/15E148 Safari/604.1';
const chromeIPhone18 =
  'Mozilla/5.0 (iPhone; CPU iPhone OS 18_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/131.0.6778.73 Mobile/15E148 Safari/604.1';
const linuxChrome =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36';
const macSafari =
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15';

test('reads the iOS version from the user agent', () => {
  assert.deepEqual(iOSversion(iPhone17), [17, 0, 0]);
  assert.deepEqual(iOSversion(iPad17), [17, 4, 1]);
  assert.deepEqual(iOSversion(chromeIPhone18), [18, 2, 0]);
});

test('gives no iOS version for desktop agents', () => {
  assert.equal(iOSversion(linuxChrome), null);
  assert.equal(iOSversion(macSafari), null);
  assert.equal(iOSversion('Mozilla/5.0 (iPhone)'), null);
});

test('reads the spoofed agent, not the host platform', () => {
  // A spoofed iOS 17 agent on a Linux host used to read as "iOS 0": the old
  // check looked at navigator.platform, which stays "Linux x86_64".
  const saved = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: { userAgent: iPhone17, platform: 'Linux x86_64' },
  });
  try {
    assert.equal(iOSversion()?.join('.'), '17.0.0');
  } finally {
    if (saved) Object.defineProperty(globalThis, 'navigator', saved);
    else delete globalThis.navigator;
  }
});
