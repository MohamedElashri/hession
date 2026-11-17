import { test } from 'node:test';
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import { createSyntaxClient } from './syntax-client.js';

class FakeWorker {
  messages = []; terminated = false;
  postMessage(message) { this.messages.push(message); }
  terminate() { this.terminated = true; }
  reply(nodes, id = this.messages.at(-1).id) { this.onmessage({ data: { id, nodes } }); }
}
const settle = () => delay(0);
function setup(t, options) {
  const workers = [], client = createSyntaxClient(() => { const w = new FakeWorker(); workers.push(w); return w; }, options);
  t.after(() => client.dispose()); return { workers, client };
}
test('a single worker serializes work, rejects stale IDs, and validates exact source text', async t => {
  const { workers, client } = setup(t); const results = [];
  client.request('first', 'js', nodes => results.push(nodes)); client.request('second', 'js', nodes => results.push(nodes));
  await settle(); assert.equal(workers.length, 1); assert.equal(workers[0].messages.length, 1);
  workers[0].reply(['first'], 999); assert.equal(results.length, 0);
  workers[0].reply(['first']); await settle(); assert.equal(workers[0].messages.length, 2);
  workers[0].reply(['altered']); assert.deepEqual(results, [['first'], null]);
});
test('cancelled work never delivers, queued cancellations are removed, and cached work needs no worker message', async t => {
  const { workers, client } = setup(t); let called = 0;
  const cancelActive = client.request('first', 'js', () => called++);
  const cancelQueued = client.request('second', 'js', () => called++); cancelQueued(); cancelActive();
  await settle(); workers[0].reply(['first']); assert.equal(called, 0); assert.equal(workers[0].messages.length, 1);
  const result = await new Promise(resolve => client.request('first', 'js', resolve)); assert.deepEqual(result, ['first']);
  assert.equal(workers[0].messages.length, 1);
});
test('timeouts terminate a stuck worker and later work can recover', async t => {
  const { workers, client } = setup(t, { timeout: 30 });
  const first = await new Promise(resolve => client.request('stuck', 'js', resolve));
  assert.equal(first, null); assert.equal(workers[0].terminated, true);
  const result = new Promise(resolve => client.request('ok', 'js', resolve)); await settle();
  workers[1].reply(['ok']); assert.deepEqual(await result, ['ok']);
});
test('worker startup failures fall back to plain text without dropping the source', async t => {
  const client = createSyntaxClient(() => Promise.reject(Error('blocked'))); t.after(() => client.dispose());
  assert.equal(await new Promise(resolve => client.request('const value = 1;', 'js', resolve)), null);
});
test('worker errors terminate the failed worker and do not poison subsequent jobs', async t => {
  const { workers, client } = setup(t); const first = new Promise(resolve => client.request('fail', 'js', resolve)); await settle();
  let prevented = false; workers[0].onerror({ preventDefault() { prevented = true; } });
  assert.equal(await first, null); assert.equal(prevented, true); assert.equal(workers[0].terminated, true);
  const next = new Promise(resolve => client.request('next', 'js', resolve)); await settle(); workers[1].reply(['next']);
  assert.deepEqual(await next, ['next']);
});
test('a late lazy worker is terminated after disposal', async t => {
  let resolveFactory; const client = createSyntaxClient(() => new Promise(resolve => { resolveFactory = resolve; }));
  t.after(() => client.dispose()); let called = false; client.request('late', 'js', () => { called = true; });
  client.dispose(); const worker = new FakeWorker(); resolveFactory(worker); await settle();
  assert.equal(worker.terminated, true); assert.equal(called, false);
});
test('queue and cache remain bounded, including duplicate queued source blocks', async t => {
  const { workers, client } = setup(t); let rejected = 0;
  for (let i = 0; i < 40; i++) client.request('same', 'js', nodes => { if (nodes == null) rejected++; });
  await settle(); assert.equal(workers.length, 1); assert.equal(rejected, 15);
  for (let i = 0; i < 25; i++) { workers[0].reply(['same']); await settle(); }
  for (let i = 0; i < 12; i++) {
    const text = `other-${i}`; client.request(text, 'js', () => {}); await settle(); workers[0].reply([text]);
  }
  const before = workers[0].messages.length; client.request('same', 'js', () => {}); await settle();
  assert.equal(workers[0].messages.length, before + 1, 'oldest cached content was evicted');
});
test('an idle worker is released while cached results remain available', async t => {
  const { workers, client } = setup(t, { idle: 10 });
  const result = new Promise(resolve => client.request('cached', 'js', resolve)); await settle(); workers[0].reply(['cached']); await result;
  await delay(25); assert.equal(workers[0].terminated, true);
  assert.deepEqual(await new Promise(resolve => client.request('cached', 'js', resolve)), ['cached']); assert.equal(workers.length, 1);
});
