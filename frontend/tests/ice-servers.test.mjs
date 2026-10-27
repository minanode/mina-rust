import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  loadIceServers,
  parseIceServers,
} from '../src/app/shared/helpers/ice-servers.helper.ts';

const turn = {
  urls: ['turn:turn.example.org:3478', 'turns:turn.example.org:5349'],
  username: 'webnode',
  credential: 'secret',
};

test('a browser-shaped list keeps the fields the node reads', () => {
  assert.deepEqual(
    parseIceServers([
      { urls: 'stun:turn.example.org:3478' },
      { ...turn, credentialType: 'password' },
    ]),
    [{ urls: ['stun:turn.example.org:3478'] }, turn],
  );
  assert.deepEqual(parseIceServers([]), []);
});

test('lists the node would refuse are dropped with a reason', () => {
  assert.equal(parseIceServers({ urls: 'stun:x' }), 'not a list');
  assert.equal(parseIceServers([null]), 'not an object');
  assert.equal(parseIceServers([{ urls: [] }]), 'a server without urls');
  assert.equal(parseIceServers([{ urls: [1] }]), 'a server without urls');
  assert.match(
    parseIceServers([{ urls: 'https://turn.example.org' }]),
    /not a STUN or TURN URL/,
  );
  assert.match(
    parseIceServers([{ urls: 'turn:turn.example.org', username: 'u' }]),
    /TURN server without a username and credential/,
  );
});

function response(status, body) {
  return Promise.resolve({
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  });
}

test('loads a list relative to the page', async () => {
  const asked = [];
  const servers = await loadIceServers(
    '/webnode-devnet/ice-servers.json',
    'https://minaweb.org/loading-web-node',
    url => {
      asked.push(url);
      return response(200, [turn]);
    },
  );
  assert.deepEqual(asked, [
    'https://minaweb.org/webnode-devnet/ice-servers.json',
  ]);
  assert.deepEqual(servers, [turn]);
});

test('no URL, a malformed one, a missing file, a bad list or a failed fetch: built-in servers', async () => {
  const base = 'https://minaweb.org/';
  assert.equal(
    await loadIceServers('', base, () => assert.fail('fetched')),
    null,
  );
  assert.equal(
    await loadIceServers('/x.json', base, () => response(404)),
    null,
  );
  assert.equal(
    await loadIceServers('/x.json', base, () =>
      response(200, { urls: 'stun:x' }),
    ),
    null,
  );
  assert.equal(
    await loadIceServers('/x.json', base, () =>
      Promise.reject(new Error('offline')),
    ),
    null,
  );
  // A malformed URL in the build: no fetch, the built-in servers.
  assert.equal(
    await loadIceServers('http://[', base, () => assert.fail('fetched')),
    null,
  );
});
