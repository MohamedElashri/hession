import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createLibraryRefresh } from './library-refresh.js';
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
test('refresh bursts share admission and later writes get one trailing refresh', async () => {
  const gate = deferred(); let calls = 0, busy = 0, peak = 0;
  const queue = createLibraryRefresh(async () => { calls++; busy++; peak = Math.max(peak, busy); if (calls === 1) await gate.promise; busy--; });
  const first = queue.request();
  for (let i=0;i<100;i++) assert.equal(queue.request(), first);
  await Promise.resolve(); assert.equal(calls, 1);
  for (let i=0;i<100;i++) assert.equal(queue.request(), first);
  gate.resolve(); await first;
  assert.equal(calls, 2); assert.equal(peak, 1);
});
test('failed refresh can retry and a trailing refresh uses the latest worker', async () => {
  const queue = createLibraryRefresh(async () => { throw new Error('busy'); });
  await assert.rejects(queue.request(), /busy/);
  const gate = deferred(); let latest = 0;
  queue.setWorker(() => gate.promise);
  const pending = queue.request(); await Promise.resolve();
  queue.setWorker(() => { latest++; }); queue.request();
  gate.resolve(); await pending; assert.equal(latest, 1);
});
test('closing suppresses queued work and StrictMode replay can reopen safely', async () => {
  let calls = 0; const gate = deferred();
  const queue = createLibraryRefresh(async () => { calls++; if (calls === 1) await gate.promise; });
  const first = queue.request(); await Promise.resolve(); queue.request(); queue.close();
  await queue.request(); gate.resolve(); await first; assert.equal(calls, 1);
  queue.open(); await queue.request(); assert.equal(calls, 2);
});

test('unmount before admission cancels work that has not started', async () => {
  let calls=0; const queue=createLibraryRefresh(()=>{calls++;});
  const pending=queue.request();queue.close();await pending;assert.equal(calls,0);
});
