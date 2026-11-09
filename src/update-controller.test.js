import test from 'node:test';
import assert from 'node:assert/strict';
import { UpdateController } from './update-controller.js';

function setup(options = {}) {
  const calls = [];
  const pkg = {
    version: '1.2.3', body: 'Changes',
    async download(report) {
      calls.push('download');
      report({ event: 'Started', data: { contentLength: 100 } });
      report({ event: 'Progress', data: { chunkLength: 40 } });
      await options.pause;
      report({ event: 'Progress', data: { chunkLength: 60 } });
      report({ event: 'Finished' });
      if (options.downloadError) throw new Error(options.downloadError);
    },
    async install() { calls.push('install'); if (options.installError) throw new Error(options.installError); },
    async close() { calls.push('close'); },
  };
  const controller = new UpdateController({
    async info() {
      calls.push('info');
      if (options.infoError) throw new Error(options.infoError);
      return { version: '1.0.0', supported: options.supported !== false, reason: 'Use a release package.' };
    },
    async check() {
      calls.push('check');
      if (options.checkError) throw new Error(options.checkError);
      return options.current ? null : pkg;
    },
    async restart() { calls.push('restart'); if (options.restartError) throw new Error(options.restartError); },
  });
  return { controller, calls, options };
}

test('opening the dialog reads local metadata without checking or installing', async () => {
  const { controller, calls } = setup();
  controller.subscribe(() => {});
  await controller.loadInfo(); await controller.install(); await controller.restart();
  assert.deepEqual(calls, ['info']);
});

test('updates finish after closing and reopening the dialog, with one install and explicit restart', async () => {
  let resume;
  const pause = new Promise(resolve => { resume = resolve; });
  const { controller, calls } = setup({ pause });
  await Promise.all([controller.check(), controller.check()]);
  assert.equal(calls.filter(call => call === 'check').length, 1);
  const stop = controller.subscribe(() => {});
  const install = controller.install();
  assert.equal(controller.getSnapshot().downloaded, 40);
  stop();
  await controller.check(); await controller.install();
  const states = [];
  const unsubscribe = controller.subscribe(() => states.push(controller.getSnapshot().phase));
  resume(); await install;
  assert.equal(controller.getSnapshot().phase, 'ready');
  assert.equal(controller.getSnapshot().downloaded, 100);
  assert.ok(states.includes('verifying'));
  assert.ok(states.includes('installing'));
  assert.equal(calls.filter(call => call === 'install').length, 1);
  assert.equal(calls.includes('restart'), false);
  await controller.check();
  assert.equal(controller.getSnapshot().phase, 'ready');
  await controller.restart();
  assert.equal(calls.at(-1), 'restart');
  unsubscribe();
});

test('invalid signatures and download errors prevent installation', async () => {
  const { controller, calls, options } = setup({ downloadError: 'Signature verification failed' });
  await controller.check(); await controller.install(); await controller.restart();
  assert.equal(controller.getSnapshot().phase, 'available');
  assert.match(controller.getSnapshot().error, /Signature/);
  assert.equal(calls.includes('install'), false);
  assert.equal(calls.includes('restart'), false);
  options.downloadError = null;
  await controller.install();
  assert.equal(controller.getSnapshot().phase, 'ready');
});

test('installation and restart failures are retryable at the appropriate step', async () => {
  const { controller, options } = setup({ installError: 'Not writable', restartError: 'Sync is running' });
  await controller.check(); await controller.install();
  assert.equal(controller.getSnapshot().phase, 'available');
  options.installError = null;
  await controller.install(); await controller.restart();
  assert.equal(controller.getSnapshot().phase, 'ready');
  assert.match(controller.getSnapshot().error, /Sync/);
  options.restartError = null;
  await controller.restart();
  assert.equal(controller.getSnapshot().phase, 'restarting');
});

test('current and unsupported installations cannot download an update', async () => {
  for (const options of [{ current: true }, { supported: false }]) {
    const { controller, calls } = setup(options);
    await controller.check(); await controller.install();
    assert.equal(calls.includes('download'), false);
    assert.equal(controller.getSnapshot().phase, options.current ? 'current' : 'idle');
    if (options.supported === false) assert.equal(calls.includes('check'), false);
  }
});

test('metadata and network failures can be retried; a recheck frees the old resource', async () => {
  const { controller, calls, options } = setup({ infoError: 'Unavailable' });
  await controller.loadInfo();
  assert.equal(calls.includes('check'), false);
  options.infoError = null; options.checkError = 'Offline';
  await controller.check();
  assert.match(controller.getSnapshot().error, /Offline/);
  options.checkError = null;
  await controller.check(); await controller.check();
  assert.equal(controller.getSnapshot().phase, 'available');
  assert.ok(calls.includes('close'));
  assert.equal(controller.getSnapshot().error, undefined);
});
