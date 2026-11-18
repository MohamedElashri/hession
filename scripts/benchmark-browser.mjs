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
const { values: options } = parseArgs({ options: { ref: { type: 'string', default: 'working' }, 'source-dir': { type: 'string' }, output: { type: 'string' }, chrome: { type: 'string' }, live: { type: 'boolean' }, unhappy: { type: 'boolean' }, 'worker-failures': { type: 'boolean' }, 'prompt-failures': { type: 'boolean' }, help: { type: 'boolean' } } });
if (options.help) { console.log('Usage: node scripts/benchmark-browser.mjs [--ref COMMIT|working] [--source-dir ARCHIVED_SRC] [--output REPORT.json] [--chrome EXECUTABLE] [--live] [--unhappy] [--worker-failures] [--prompt-failures]'); process.exit(0); }
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
  if (options['source-dir']) cpSync(resolve(options['source-dir']), join(site, 'src'), { recursive: true,
    filter: path => !resolve(path).startsWith(resolve(options['source-dir'], 'dev')) });
  assert(!existsSync(join(site, 'src/dev/fixtures.local.json')));
  assert(!existsSync(join(site, 'plan.md')));
  symlinkSync(join(repository, 'node_modules'), join(site, 'node_modules'), 'dir');
  writeFileSync(join(site, 'index.html'), '<!doctype html><html data-theme="light"><head><meta charset="UTF-8"></head><body><main id="root"></main><script type="module" src="/performance.jsx"></script></body></html>');
  writeFileSync(join(site, 'performance.jsx'), `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import ToolActivity from './src/components/ToolActivity.jsx';
import { LiveTail } from './src/components/LiveView.jsx';
import useIndexing from './src/useIndexing.js';
import { createSyntaxClient } from './src/syntax-client.js';
import { parsePrompt } from './src/transcript.js';
import '@fontsource-variable/geist';
import '@fontsource-variable/geist-mono';
import './src/styles/base.css';
import './src/styles/app.css';
import './src/styles/components.css';
let root = createRoot(document.getElementById('root'));
let cards;
window.prepareFixture = (size, count, scenario = "indexed") => {
  const command = '# Synthetic recorded command\\nprintf "%s\\\\n" "' + 'x'.repeat(size) + '"';
  const args = JSON.stringify({ cmd: command, workdir: '/synthetic/project', timeout: 30 });
  cards = Array.from({length: count}, (_, index) => ({id:index, tool_name:'exec_command', command_text:command, arguments_json:args, result_text:'{"ok":true}', status:'completed'}));
  if (scenario !== 'indexed') {
    const input = index => scenario.startsWith('malformed') ? '{"patch":"' + 'x'.repeat(size) + (index ?? '')
      : JSON.stringify(scenario.startsWith('path') ? {file_path:'/synthetic/project'+(index ?? '')+'.rs',content:'x'.repeat(size)} : {code:'x'.repeat(size)+(index == null ? '' : '//'+index),language:'python'});
    // Share the repeated source string before the initial heap measurement.
    // Otherwise string deduplication during lookup can subtract input memory
    // and masquerade as negative component retention. Unique cases are controls.
    const shared = scenario.endsWith('-unique') ? null : input(null);
    cards = cards.map((tool,index) => ({ ...tool, command_text:null, status:'failed', arguments_json:shared ?? input(index) }));
  }
  // Flatten synthetic concatenated strings before the initial heap snapshot,
  // as IPC-decoded strings would already be. Input allocation is not card memory.
  for (const tool of cards) {
    tool.arguments_json.indexOf(String.fromCharCode(10));
    if (typeof tool.command_text === 'string') tool.command_text.indexOf(String.fromCharCode(10));
  }
};
window.mountFixture = () => {
  const start = performance.now();
  flushSync(() => root.render(<>{cards.map(tool => <ToolActivity key={tool.id} tool={tool}/>)}</>));
  return {commitMs:performance.now()-start, cards:document.querySelectorAll('.tool-activity').length, expandedBodies:document.querySelectorAll('.tool-activity-body').length, domNodes:document.querySelectorAll('*').length};
};
window.clearFixture = () => { flushSync(() => root.unmount()); root = createRoot(document.getElementById('root')); };
await document.fonts.ready;
window.runPromptFaults = () => {
  const text = '<USER_REQUEST>x'.repeat(16000) + '</OTHER>', samples = [];
  for (let run = 0; run < 10; run++) {
    const start = performance.now();
    const prompt = parsePrompt(text);
    const elapsed = performance.now()-start;
    if (prompt.text !== text) throw new Error('Truncated prompt source changed');
    if (run >= 3) samples.push(elapsed);
  }
  samples.sort((a,b) => a-b);
  return {characters:text.length,openers:16000,medianMs:samples[3],samplesMs:samples};
};
window.runWorkerFaults = async () => {
  let starts = 0;
  const url = URL.createObjectURL(new Blob(['self.onmessage = () => {};'], {type:'text/javascript'}));
  const client = createSyntaxClient(() => { starts++; return new Worker(url); });
  const start = performance.now();
  try {
    const results = await Promise.all(Array.from({length:8}, (_,i) => new Promise(resolve => client.request('const item'+i+'=1;', 'js', resolve))));
    if (!results.every(result => result === null)) throw new Error('Failure did not fall back to plain text');
    return {blocks:8, timeoutMs:2500, workerStarts:starts, elapsedMs:performance.now()-start};
  } finally { client.dispose(); URL.revokeObjectURL(url); }
};
let liveSequence = 0, livePage = 0, liveMode = '', liveDone, liveCursor;
const callbacks = new Map(), listeners = new Set(); let callbackId = 0;
const liveEvent = id => ({id,ordinal:id*11,position:id-1,kind:'message',role:'assistant',occurred_at:'2026-11-18T00:00:00Z',parts:[{kind:'text',text:'Event '+id+' '+ 'x'.repeat(4096)}],tools:[]});
let dateFormats = 0;
const formatDate = Date.prototype.toLocaleString;
Date.prototype.toLocaleString = function(...args) { dateFormats++; return formatDate.apply(this,args); };
const openLive = () => {};
function LiveFixture() {
  const [revision, setRevision] = React.useState(0);
  window.bumpLive = () => setRevision(value => value+1);
  return <div data-fixture-revision={revision}><LiveTail session={{id:1,title:'Synthetic live session',agent:'codex'}} available onSelectSession={openLive}/></div>;
}
window.prepareLive = () => {
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback) { const id=++callbackId; callbacks.set(id,callback); return id; },
    async invoke(command,args) {
      if (command === 'plugin:event|listen') { listeners.add(args.handler); return args.handler; }
      if (command === 'plugin:event|unlisten') return;
      if (command !== 'session_delta') throw new Error('Unexpected fixture command: '+command);
      liveCursor = args.cursor?.seq ?? null;
      let entries = [], reset = false, hasMore = false;
      if (livePage < 3) {
        entries = Array.from({length:100},(_,i)=>liveEvent(livePage*100+i+1));
        reset = livePage===0; livePage++; hasMore=livePage<3;
      } else if (liveMode === 'append') entries = [liveEvent(301)];
      else if (liveMode === 'replay') entries = Array.from({length:100},()=>liveEvent(301));
      else if (liveMode === 'duplicate-new') entries = Array.from({length:100},()=>liveEvent(302));
      const result = {entries,reset,hasMore,deleted:false,cursor:{identity:'synthetic',epoch:1,seq:++liveSequence,sessionId:1}};
      liveDone?.(); return result;
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {unregisterListener(_event,id) {listeners.delete(id);callbacks.delete(id);}};
  flushSync(()=>root.render(<LiveFixture/>));
};
window.inspectLive = () => ({rows:document.querySelectorAll('[data-live-entry]').length,error:document.querySelector('.live-error')?.textContent});
window.refreshLiveParent = () => {
  const before=dateFormats,start=performance.now(); flushSync(()=>window.bumpLive());
  return {commitMs:performance.now()-start,dateFormats:dateFormats-before};
};
window.runLiveDelta = mode => new Promise(resolve => {
  liveMode=mode; const before=dateFormats;
  liveDone = () => {
    liveDone=null;
    requestAnimationFrame(()=>requestAnimationFrame(()=>resolve({mode,dateFormats:dateFormats-before,rows:window.inspectLive().rows,requestedCursor:liveCursor,sequence:liveSequence,pending:document.querySelector('.live-tail-controls .text-button')?.textContent})));
  };
  for (const id of listeners) callbacks.get(id)?.({event:'library-changed',payload:{}});
});
let indexMetrics, startResolvers=[];
const indexChanged = async () => { indexMetrics.refreshes++; };
function IndexFixture() {
  const [ready,setReady]=React.useState(true); window.setIndexReady=setReady;
  useIndexing({ready,onChanged:indexChanged}); return null;
}
window.runIndexingProbe = async () => {
  indexMetrics={starts:0,refreshes:0,statusReads:0,revisionReads:0};startResolvers=[];
  const invoke=window.__TAURI_INTERNALS__.invoke;
  window.__TAURI_INTERNALS__.invoke=async (command,args) => {
    if(command==='start_indexing'){indexMetrics.starts++;return new Promise(resolve=>startResolvers.push(resolve));}
    if(command==='pause_indexing')return {phase:args.paused?'paused':'active'};
    if(command==='indexing_status'){indexMetrics.statusReads++;return {phase:'active'};}
    if(command==='observed_library_revision'){indexMetrics.revisionReads++;return {identity:'synthetic-index',revision:1};}
    return invoke(command,args);
  };
  flushSync(()=>root.render(<IndexFixture/>));
  flushSync(()=>window.setIndexReady(false));flushSync(()=>window.setIndexReady(true));
  for(let step=0;step<5;step++){
    for(const resolve of startResolvers.splice(0))resolve({phase:'active'});
    await new Promise(resolve=>setTimeout(resolve,0));
  }
  for(let i=0;i<100;i++)for(const id of listeners)callbacks.get(id)?.({event:'library-changed',payload:{revision:{identity:'synthetic-index',revision:1}}});
  await new Promise(resolve=>setTimeout(resolve,3300));
  const result={...indexMetrics}; flushSync(()=>root.unmount());root=createRoot(document.getElementById('root')); return result;
};
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
      if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails.exception?.description || message.params.exceptionDetails.text);
    }
  });
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const id = ++next;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error('CDP timeout: ' + method)); }, 30000);
    pending.set(id, { resolve, reject, timer });
    chrome.stdio[3].write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + '\0');
  });
  let { targetId } = await send('Target.createTarget', { url: 'http://127.0.0.1:' + port });
  let { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
  await send('Runtime.enable', {}, sessionId);
  const evaluate = async expression => {
    const response = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, sessionId);
    assert(!response.exceptionDetails, JSON.stringify(response.exceptionDetails));
    return response.result.value;
  };
  const ready = async () => {
    for (let attempt = 0; !await evaluate('!!window.benchmarkReady'); attempt++) {
      assert.equal(errors.length, 0, errors.join('\n'));
      assert(attempt < 300, 'Benchmark page failed to load within 30 seconds');
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  };
  await ready();
  const cases = [];
  if (options.live) {
    await evaluate('window.prepareLive()');
    let ready;
    for (let attempt = 0; attempt < 300; attempt++) {
      ready = await evaluate('window.inspectLive()');
      assert(!ready.error, ready.error);
      if (ready.rows===300) break;
      await new Promise(resolve => setTimeout(resolve,100));
    }
    assert.equal(ready.rows,300);
    await evaluate("document.querySelector('.live-tail-controls .text-button').click()");
    await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    const samples=[],formats=[];
    for (let run=0;run<10;run++) {
      const result=await evaluate('window.refreshLiveParent()');
      if (run>=3) {samples.push(result.commitMs);formats.push(result.dateFormats);}
      await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    }
    samples.sort((a,b)=>a-b);
    cases.push({scenario:'live-parent-refresh',rows:300,medianCommitMs:samples[3],samplesMs:samples,dateFormats:formats});
    for (const mode of ['empty','append','replay','duplicate-new']) {
      const result=await evaluate('window.runLiveDelta('+JSON.stringify(mode)+')');
      assert.equal(result.rows,300); cases.push({scenario:'live-'+mode,...result});
    }
    await evaluate('window.clearFixture()');
    cases.push({scenario:'indexing-event-and-poll',...await evaluate('window.runIndexingProbe()')});
  } else {
  const scenarios = options.unhappy ? ['malformed','path','code','malformed-unique','path-unique','code-unique'].map(scenario => [scenario,1024 * 1024,50])
    : [['indexed', 1024, 200], ['indexed', 64 * 1024, 200], ['indexed', 1024 * 1024, 50]];
  for (const [scenario, size, count] of scenarios) {
    // Keep module-level caches from one workload out of the next workload's
    // initial heap. Each case warms its own fresh page before measured samples.
    if (cases.length) {
      await send('Target.closeTarget', { targetId });
      ({ targetId } = await send('Target.createTarget', { url: 'http://127.0.0.1:' + port }));
      ({ sessionId } = await send('Target.attachToTarget', { targetId, flatten: true }));
      await send('Runtime.enable', {}, sessionId);
      await ready();
    }
    await evaluate('window.prepareFixture(' + size + ',' + count + ',' + JSON.stringify(scenario) + ')');
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
    cases.push({ scenario, argumentPayload: size, cards: count, medianCommitMs: samples[Math.floor(samples.length / 2)], p95CommitMs: samples[Math.ceil(samples.length * .95) - 1], retainedHeapBytes: after.usedSize - before.usedSize, domNodes: mounted.domNodes, samplesMs: samples });
  }
  }
  assert.equal(errors.length, 0, errors.join('\n'));
  const workerFailure = options['worker-failures'] ? await evaluate('window.runWorkerFaults()') : undefined;
  const promptFailure = options['prompt-failures'] ? await evaluate('window.runPromptFaults()') : undefined;
  const report = { ref, source: options['source-dir'] ? 'archived source snapshot' : 'repository source', workerFailure, promptFailure, browser: await send('Browser.getVersion'), scenario: options.live ? 'production LiveTail, synthetic IPC' : options.unhappy ? 'production component, collapsed failed/fallback calls' : 'production component, collapsed recorded commands', cases };
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
