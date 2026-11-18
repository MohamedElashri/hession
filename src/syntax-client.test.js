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
test('active cancellation frees a stuck worker and queued cancellation prevents delivery', async t => {
  const { workers, client } = setup(t); let called = 0;
  const cancelActive = client.request('first', 'js', () => called++);
  const cancelQueued = client.request('second', 'js', () => called++);
  const next = new Promise(resolve => client.request('third', 'js', resolve));
  await settle(); cancelQueued(); cancelActive(); await settle();
  assert.equal(workers[0].terminated, true);
  workers[0].reply(['first']); assert.equal(called, 0);
  workers[1].reply(['third']); assert.deepEqual(await next, ['third']);
  assert.deepEqual(await new Promise(resolve => client.request('third', 'js', resolve)), ['third']);
  assert.equal(workers[1].messages.length, 1);
});
test('timeouts terminate a stuck worker and later work can recover', async t => {
  const { workers, client } = setup(t, { timeout: 30, retryDelay: 0 });
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
  const { workers, client } = setup(t, { retryDelay: 0 }); const first = new Promise(resolve => client.request('fail', 'js', resolve)); await settle();
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
  workers[0].reply(['same']); await settle();
  assert.equal(workers[0].messages.length, 1, 'queued duplicates reuse the completed tree');
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

test('a stuck worker drains the queue once and retries only after the cooldown', async t => {
  let now = 1000; t.mock.method(performance, 'now', () => now);
  const { workers, client } = setup(t, { timeout: 20 });
  const result = await Promise.all(Array.from({ length: 25 }, (_, index) => new Promise(resolve => client.request(`block-${index}`, 'js', resolve))));
  assert.deepEqual(result, Array(25).fill(null));
  assert.equal(workers.length, 1); assert.equal(workers[0].terminated, true);
  assert.equal(await new Promise(resolve => client.request('later', 'js', resolve)), null);
  assert.equal(workers.length, 1);
  now += 5001;
  const next = new Promise(resolve => client.request('later', 'js', resolve)); await settle();
  workers[1].reply(['later']); assert.deepEqual(await next, ['later']);
});
test('blocked worker startup does not retry for each queued block', async t => {
  let attempts = 0;
  const client = createSyntaxClient(() => { attempts++; return Promise.reject(Error('blocked')); });
  t.after(() => client.dispose());
  const result = await Promise.all(Array.from({ length: 25 }, (_, index) => new Promise(resolve => client.request(`blocked-${index}`, 'js', resolve))));
  assert.deepEqual(result, Array(25).fill(null)); assert.equal(attempts, 1);
});
test('invalid trees and explicit plain-text fallbacks are cached with bounded eviction', async t => {
  const { workers, client } = setup(t);
  for (const nodes of [null, ['wrong source']]) {
    const source = nodes ? 'invalid' : 'unsupported';
    const result = new Promise(resolve => client.request(source, 'js', resolve)); await settle();
    workers[0].reply(nodes); assert.equal(await result, null);
    const sent = workers[0].messages.length;
    assert.equal(await new Promise(resolve => client.request(source, 'js', resolve)), null);
    assert.equal(workers[0].messages.length, sent);
  }
  for (let i = 0; i < 9; i++) {
    const result = new Promise(resolve => client.request(`fallback-${i}`, 'js', resolve)); await settle();
    workers[0].reply(null); await result;
  }
  const sent = workers[0].messages.length;
  client.request('unsupported', 'js', () => {}); await settle();
  assert.equal(workers[0].messages.length, sent + 1, 'negative cache also evicts old entries');
});
test('cached text is usable even when the work queue is full', async t => {
  const { workers, client } = setup(t);
  const ready = new Promise(resolve => client.request('cached', 'js', resolve)); await settle();
  workers[0].reply(['cached']); await ready;
  for (let i = 0; i < 25; i++) client.request(`pending-${i}`, 'js', () => {});
  assert.deepEqual(await new Promise(resolve => client.request('cached', 'js', resolve)), ['cached']);
});
test('replacement jobs share pending startup across a burst of cancellations', async t => {
  let resolveFactory, starts = 0;
  const client = createSyntaxClient(() => { starts++; return new Promise(resolve => { resolveFactory = resolve; }); });
  t.after(() => client.dispose()); let staleCalled = false;
  for (let i = 0; i < 25; i++) client.request(`stale-${i}`, 'js', () => { staleCalled = true; })();
  const next = new Promise(resolve => client.request('current', 'js', resolve));
  assert.equal(starts, 1);
  const current = new FakeWorker(); resolveFactory(current); await settle();
  assert.equal(current.terminated, false); assert.equal(current.messages.length, 1);
  current.reply(['current']); assert.deepEqual(await next, ['current']); assert.equal(staleCalled, false);
});
test('late startup after disposal cannot replace a fresh worker', async t => {
  const pending = [];
  const client = createSyntaxClient(() => new Promise(resolve => pending.push(resolve)));
  t.after(() => client.dispose()); let staleCalled = false;
  client.request('stale', 'js', () => { staleCalled = true; }); client.dispose();
  const next = new Promise(resolve => client.request('current', 'js', resolve));
  const stale = new FakeWorker(), current = new FakeWorker();
  pending[0](stale); pending[1](current); await settle();
  assert.equal(stale.terminated, true); assert.equal(current.terminated, false);
  current.reply(['current']); assert.deepEqual(await next, ['current']); assert.equal(staleCalled, false);
});

test('synchronous startup and message transport failures release every pending block', async t => {
  for (const failure of ['startup', 'post', 'decode']) {
    let attempts = 0;
    const worker = new FakeWorker();
    if (failure === 'post') worker.postMessage = () => { throw Error('Synthetic clone failure'); };
    const client = createSyntaxClient(() => {
      attempts++;
      if (failure === 'startup') throw Error('Synthetic CSP failure');
      return worker;
    });
    t.after(() => client.dispose());
    const pending = Promise.all(Array.from({ length: 25 }, (_, i) => new Promise(resolve => client.request(`block-${i}`, 'js', resolve))));
    if (failure === 'decode') { await settle(); worker.onmessageerror(); }
    assert.deepEqual(await pending, Array(25).fill(null), failure);
    assert.equal(attempts, 1, failure);
    if (failure !== 'startup') assert.equal(worker.terminated, true);
  }
});
test('rejected shared startup after cancellation can recover after cooldown', async t => {
  let now = 1000; t.mock.method(performance, 'now', () => now);
  let rejectFirst, attempts = 0;
  const worker = new FakeWorker();
  const client = createSyntaxClient(() => ++attempts === 1 ? new Promise((_, reject) => { rejectFirst = reject; }) : worker);
  t.after(() => client.dispose());
  let cancelledCalled = false;
  client.request('cancelled', 'js', () => { cancelledCalled = true; })();
  const current = new Promise(resolve => client.request('current', 'js', resolve));
  rejectFirst(Error('Synthetic import failure'));
  assert.equal(await current, null); assert.equal(cancelledCalled, false); assert.equal(attempts, 1);
  now += 5001;
  const recovered = new Promise(resolve => client.request('current', 'js', resolve)); await settle();
  worker.reply(['current']); assert.deepEqual(await recovered, ['current']); assert.equal(attempts, 2);
});

test('disposal suppresses deferred cached, rejected, and drained callbacks', async t => {
  const { workers, client }=setup(t);
  const ready=new Promise(resolve=>client.request('cached','js',resolve));await settle();workers[0].reply(['cached']);await ready;
  let calls=0;
  client.request('cached','js',()=>calls++);
  client.request('not highlighted','plaintext',()=>calls++);
  client.request('failure','js',()=>{});
  client.request('queued','js',()=>calls++);
  workers[0].onerror({});client.dispose();await settle();
  assert.equal(calls,0);
  const fresh=new Promise(resolve=>client.request('fresh','js',resolve));await settle();
  workers[1].reply(['fresh']);assert.deepEqual(await fresh,['fresh']);
});
