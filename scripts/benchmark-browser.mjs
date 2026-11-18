// Production React component measurements in an isolated headless browser.
// No Tauri IPC, agent stores, personal preview fixtures, or existing browser profile.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const { values: options } = parseArgs({ options: { ref: { type: 'string', default: 'working' }, output: { type: 'string' }, chrome: { type: 'string' }, help: { type: 'boolean' } } });
if (options.help) { console.log('Usage: node scripts/benchmark-browser.mjs [--ref COMMIT|working] [--output REPORT.json] [--chrome EXECUTABLE]'); process.exit(0); }
const ref = options.ref || 'working';
const scratch = mkdtempSync(join(tmpdir(), 'hession-browser-perf-'));
const site = join(scratch, 'site');
mkdirSync(site);
let chrome, server;
try {
  if (ref === 'working') {
    const tracked = execFileSync('git', ['ls-files', '-z', '--', 'src', 'package.json', 'brand.json', 'vite.config.js'], { cwd: repository }).toString().split('\0').filter(Boolean);
    for (const path of tracked.filter(path => !path.startsWith('src/dev/'))) {
      const target = join(site, path);
      mkdirSync(dirname(target), { recursive: true });
      cpSync(join(repository, path), target);
    }
  } else {
    const archive = join(scratch, 'source.tar');
    execFileSync('git', ['archive', '--format=tar', '--output', archive, ref, 'src', 'package.json', 'brand.json', 'vite.config.js'], { cwd: repository });
    execFileSync('tar', ['-xf', archive, '-C', site]);
    rmSync(join(site, 'src/dev'), { recursive: true, force: true });
  }
  assert(!existsSync(join(site, 'src/dev/fixtures.local.json')));
  assert(!existsSync(join(site, 'plan.md')));
  symlinkSync(join(repository, 'node_modules'), join(site, 'node_modules'), 'dir');
  writeFileSync(join(site, 'index.html'), '<!doctype html><html data-theme="light"><head><meta charset="UTF-8"></head><body><main id="root"></main><script type="module" src="/performance.jsx"></script></body></html>');
  writeFileSync(join(site, 'performance.jsx'), `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import ToolActivity from './src/components/ToolActivity.jsx';
import '@fontsource-variable/geist';
import '@fontsource-variable/geist-mono';
import './src/styles/base.css';
import './src/styles/app.css';
import './src/styles/components.css';
let root = createRoot(document.getElementById('root'));
let cards;
window.prepareFixture = (size, count) => {
  const command = '# Synthetic recorded command\\nprintf "%s\\\\n" "' + 'x'.repeat(size) + '"';
  const args = JSON.stringify({ cmd: command, workdir: '/synthetic/project', timeout: 30 });
  cards = Array.from({length: count}, (_, index) => ({id:index, tool_name:'exec_command', command_text:command, arguments_json:args, result_text:'{"ok":true}', status:'completed'}));
};
window.mountFixture = () => {
  const start = performance.now();
  flushSync(() => root.render(<>{cards.map(tool => <ToolActivity key={tool.id} tool={tool}/>)}</>));
  return {commitMs:performance.now()-start, cards:document.querySelectorAll('.tool-activity').length, expandedBodies:document.querySelectorAll('.tool-activity-body').length, domNodes:document.querySelectorAll('*').length};
};
window.clearFixture = () => { flushSync(() => root.unmount()); root = createRoot(document.getElementById('root')); };
await document.fonts.ready;
window.benchmarkReady = true;
`);
  await build({ root: site, configFile: join(site, 'vite.config.js'), logLevel: 'silent', build: { outDir: 'dist', sourcemap: true } });
  const dist = join(site, 'dist');
  const mime = { '.html': 'text/html', '.js': 'application/javascript', '.css': 'text/css', '.woff2': 'font/woff2' };
  server = createServer((request, response) => {
    try {
      const path = resolve(dist, '.' + new URL(request.url, 'http://localhost').pathname.replace(/\/$/, '/index.html'));
      assert(path.startsWith(dist + '/'));
      response.setHeader('Content-Type', mime[extname(path)] || 'application/octet-stream');
      response.end(readFileSync(path));
    } catch { response.writeHead(404); response.end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  chrome = spawn(options.chrome || 'google-chrome', ['--headless=new', '--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-extensions', '--disable-sync', '--remote-debugging-pipe', '--user-data-dir=' + join(scratch, 'chrome'), '--window-size=1280,900', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'] });
  let buffer = Buffer.alloc(0), next = 0;
  const pending = new Map(), errors = [];
  chrome.stderr.on('data', () => {});
  chrome.stdio[4].on('data', chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    for (let boundary; (boundary = buffer.indexOf(0)) >= 0;) {
      const message = JSON.parse(buffer.subarray(0, boundary).toString());
      buffer = buffer.subarray(boundary + 1);
      const request = pending.get(message.id);
      if (request) { pending.delete(message.id); clearTimeout(request.timer); message.error ? request.reject(new Error(JSON.stringify(message.error))) : request.resolve(message.result); }
      if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails.text);
    }
  });
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const id = ++next;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error('CDP timeout: ' + method)); }, 30000);
    pending.set(id, { resolve, reject, timer });
    chrome.stdio[3].write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + '\0');
  });
  const { targetId } = await send('Target.createTarget', { url: 'http://127.0.0.1:' + port });
  const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
  await send('Runtime.enable', {}, sessionId);
  const evaluate = async expression => {
    const response = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, sessionId);
    assert(!response.exceptionDetails, JSON.stringify(response.exceptionDetails));
    return response.result.value;
  };
  for (let attempt = 0; !await evaluate('!!window.benchmarkReady'); attempt++) {
    assert(attempt < 100, 'Benchmark page failed to load');
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  const cases = [];
  for (const [size, count] of [[1024, 200], [64 * 1024, 200], [1024 * 1024, 50]]) {
    await evaluate('window.prepareFixture(' + size + ',' + count + ')');
    await send('HeapProfiler.collectGarbage', {}, sessionId);
    const before = await send('Runtime.getHeapUsage', {}, sessionId);
    const samples = [];
    let mounted;
    for (let iteration = 0; iteration < 10; iteration++) {
      mounted = await evaluate('window.mountFixture()');
      assert.equal(mounted.cards, count); assert.equal(mounted.expandedBodies, 0);
      await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
      if (iteration >= 3) samples.push(mounted.commitMs);
      if (iteration < 9) await evaluate('window.clearFixture()');
    }
    await send('HeapProfiler.collectGarbage', {}, sessionId);
    const after = await send('Runtime.getHeapUsage', {}, sessionId);
    await evaluate('window.clearFixture()');
    samples.sort((a, b) => a - b);
    cases.push({ argumentPayload: size, cards: count, medianCommitMs: samples[Math.floor(samples.length / 2)], p95CommitMs: samples[Math.ceil(samples.length * .95) - 1], retainedHeapBytes: after.usedSize - before.usedSize, domNodes: mounted.domNodes, samplesMs: samples });
  }
  assert.equal(errors.length, 0, errors.join('\n'));
  const report = { ref, browser: await send('Browser.getVersion'), scenario: 'production component, collapsed recorded commands', cases };
  const output = JSON.stringify(report, null, 2) + '\n';
  if (options.output) writeFileSync(resolve(options.output), output);
  process.stdout.write(output);
} finally {
  if (chrome && chrome.exitCode == null) {
    await new Promise(resolve => {
      const timer = setTimeout(() => { chrome.kill('SIGKILL'); resolve(); }, 3000);
      chrome.once('exit', () => { clearTimeout(timer); resolve(); });
      chrome.kill('SIGTERM');
    });
  }
  if (server) await new Promise(resolve => server.close(resolve));
  rmSync(scratch, { recursive: true, force: true });
}
