#!/usr/bin/env node
// Fake native transport peer: no Marrow image or store is opened.
import { createServer } from 'node:net';
import { existsSync, appendFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import * as M from './marrow-supervisor.mjs';

const root = process.env.MARROW_CLOSE_ROOT;
const mode = process.env.MARROW_CLOSE_CASE;
if (!root) throw new Error('missing controlled root');
const identity = '34'.repeat(32);
const session = '56'.repeat(32);
// Bound under the system temp root: a socket path inside the fixture root can
// exceed the platform's sun_path limit.
const socket = join(mkdtempSync(join(tmpdir(), 'marrow-close-')), 's');
const note = value => appendFileSync(join(root, 'child-events.jsonl'), JSON.stringify(value) + '\n');
const peers = new Set();
let finished = false;
const server = createServer({ allowHalfOpen: true }, peer => {
  peers.add(peer);
  let received = Buffer.alloc(0);
  let greeted = false;
  peer.on('error', error => note({ event: 'peer-error', message: error.message }));
  peer.on('close', () => peers.delete(peer));
  peer.on('end', () => {
    note({ event: 'hangup', at: performance.now() });
    peer.end();
  });
  peer.on('data', chunk => {
    received = Buffer.concat([received, chunk]);
    if (received.length < 4) return;
    const length = received.readUInt32BE(0);
    if (length > 4096) { finish(3, 'oversized-hello'); return; }
    if (received.length < length + 4) return;
    const hello = M.parseCanonical(received.subarray(5, length + 4));
    received = received.subarray(length + 4);
    if (greeted) {
      note({ event: 'request' });
      if (hello.kind !== 'request') { finish(3, 'wrong-request'); return; }
      peer.write(M.encodeFrame({ kind: 'value', data: 42n,
        turn: mode === 'protocol' ? hello.turn + 1n : hello.turn }));
      return;
    }
    if (hello.kind !== 'hello' || hello.nonce !== process.env.MARROW_RUNNER_NONCE) {
      finish(3, 'wrong-hello'); return;
    }
    greeted = true;
    note({ event: 'hello' });
    peer.write(M.encodeFrame(mode === 'startup'
      ? { interface: identity, kind: 'activation_uncertain', session,
          code: 'store.activation_uncertain', instance: '78'.repeat(16) }
      : { interface: identity, kind: 'ready', session }));
  });
});
function finish(code, reason) {
  if (finished) return;
  finished = true;
  clearInterval(releasePoll);
  clearTimeout(expiry);
  note({ event: 'natural-exit', code, reason, at: performance.now() });
  for (const peer of peers) peer.destroy();
  server.close();
  process.exitCode = code;
}
// These timers hold the peer alive past EOF; only the parent release or this finite
// expiry ends it, so an exit here is never the supervisor's doing.
const releasePoll = setInterval(() => {
  if (existsSync(join(root, 'release'))) finish(0, 'parent-release');
}, 20);
const expiry = setTimeout(() => finish(4, 'release-deadline'), 10_000);
server.listen(socket, () => {
  note({ event: 'listening', pid: process.pid });
  process.stdout.write(M.encodeCanonical({ interface: identity, session, socket }) + '\n');
});
