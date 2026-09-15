// Controlled close-policy fixture. No Marrow image or engine is opened.
import assert from 'node:assert/strict';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import * as M from './marrow-supervisor.mjs';

const root = resolve(process.env.MARROW_CLOSE_ROOT ?? '.');
const mode = process.env.MARROW_CLOSE_CASE ?? 'ordinary';
process.env.MARROW_CLOSE_ROOT = root;
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const eventLog = () => readFileSync(join(root, 'child-events.jsonl'), 'utf8');
const release = why => writeFileSync(join(root, 'release'), why + '\n');

// The startup peer never sees a hangup, so release it once it has authenticated.
const startupRelease = mode === 'startup' ? (async () => {
  for (let i = 0; i < 400; i++) {
    const path = join(root, 'child-events.jsonl');
    if (existsSync(path) && readFileSync(path, 'utf8').includes('"hello"')) {
      await delay(300);
      release('authenticated startup release');
      return;
    }
    await delay(20);
  }
})() : null;

let session;
try {
  session = await M.launch({ runner: join(root, 'child.mjs'), image: join(root, 'fake.image'),
    store: join(root, 'fake-store'), expectedIdentity: '34'.repeat(32), log: () => {} });
} catch (error) {
  release('release after failed launch');
  await startupRelease;
  if (mode !== 'startup') throw error;
  assert.ok(error instanceof M.ActivationUncertainError);
  assert.equal(error.instance, '78'.repeat(16));
  assert.equal(error.cleanup.kind, 'exited');
  assert.equal(error.cleanup.code, 0);
  assert.equal(error.cleanup.signal, null);
  assert.ok(eventLog().includes('parent-release'), 'controlled startup child reached release');
  console.log('DRIVER: all passed');
}

if (session !== undefined) {
  assert.notEqual(mode, 'startup', 'startup uncertainty must reject launch');
  const child = session.child;
  const started = performance.now();
  const at = () => performance.now() - started;
  let exit = null;
  let streamsClosed = false;
  const exited = new Promise(resolve => child.once('exit', (code, signal) => {
    exit = { code, signal, milliseconds: at() };
    resolve();
  }));
  const closed = new Promise(resolve => child.once('close', () => {
    streamsClosed = true;
    resolve();
  }));
  const settled = {};
  const watch = (name, promise) => Promise.resolve(promise).then(
    () => { settled[name] = 'fulfilled'; }, () => { settled[name] = 'rejected'; });

  let callError;
  let value;
  try { value = await session.call('12'.repeat(32), [], value => value); }
  catch (error) { callError = error; }
  if (mode === 'abort') session.terminate();
  const firstPromise = session.close();
  const secondPromise = session.close();
  let firstError;
  let secondError;
  firstPromise.catch(error => { firstError = error; });
  secondPromise.catch(error => { secondError = error; });
  const first = watch('first', firstPromise);
  const second = watch('second', secondPromise);
  await delay(0);
  const immediate = { first: settled.first ?? null, second: settled.second ?? null };
  // Let the supervisor's own exit deadline expire before releasing the peer, so the
  // observed disposition is the deadline's rather than a natural exit's.
  await delay(Math.max(0, 2300 - at()));
  const beforeRelease = at();
  release('parent releases controlled peer');
  let terminalTimer;
  try {
    await Promise.race([Promise.all([exited, closed, first, second]),
      new Promise(resolve => { terminalTimer = setTimeout(resolve, 11_000); })]);
  } finally {
    clearTimeout(terminalTimer);
  }

  const events = eventLog().trim().split('\n').map(line => JSON.parse(line));
  assert.ok(exit !== null && streamsClosed, 'terminal child and pipe custody required');
  assert.ok(events.some(event => event.event === 'hello'), 'authenticated launch reached');
  assert.ok(events.some(event => event.event === 'request'), 'actual request reached peer');
  if (mode === 'protocol') {
    assert.ok(callError instanceof M.MarrowLossError);
    assert.equal(callError.loss, M.LOSS.OUTCOME_UNKNOWN);
  } else {
    assert.equal(value, 42n, 'known reply survives cleanup disposition');
  }
  if (mode === 'abort') {
    assert.equal(exit.signal, 'SIGKILL', 'explicit abort remains abrupt');
    assert.equal(settled.first, 'fulfilled', 'explicit exit was observed');
  } else {
    assert.ok(events.some(event => event.event === 'hangup'), 'close reached child EOF');
    assert.equal(immediate.second, null, 'second close must share pending settlement');
    assert.equal(exit.signal, null, 'native close must not force a signal');
    assert.equal(exit.code, 0, 'controlled peer must exit after release');
    assert.ok(exit.milliseconds >= beforeRelease);
    assert.equal(settled.first, settled.second, 'both callers share one disposition');
    assert.ok(settled.first && settled.second, 'both settlement calls observed');
    assert.equal(firstPromise, secondPromise, 'close returns the memoized public promise');
    assert.ok(firstError instanceof M.MarrowCleanupError, 'deadline is typed cleanup uncertainty');
    assert.equal(firstError, secondError, 'both callers receive the same cleanup error');
    assert.equal(firstError.cleanup.kind, 'unconfirmed');
    assert.equal(firstError.cleanup.pid, child.pid);
    assert.equal(session.close(), firstPromise, 'late exit must not replace the observation');
  }
  console.log('DRIVER: all passed');
}
