// Synthetic failure/fallback probes. Optional argument: archived src directory.
import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const source = pathToFileURL(resolve(process.argv[2] || 'src') + '/');
const { toolPresentation } = await import(new URL('tool-presentation.js', source));
const { createSyntaxClient } = await import(new URL('syntax-client.js', source));
const reports = [];
const median = samples => [...samples].sort((a, b) => a - b)[Math.floor(samples.length / 2)];
for (const [scenario, tool] of [
  ['malformed-arguments', { arguments_json: '{"patch":"' + 'x'.repeat(1024 * 1024) }],
  ['fallback-file-path', { arguments_json: JSON.stringify({ file_path: '/synthetic/project.rs', content: 'x'.repeat(1024 * 1024) }) }],
  ['failed-code-call', { status: 'failed', arguments_json: JSON.stringify({ code: 'x'.repeat(1024 * 1024), language: 'python' }) }],
]) {
  const samples = [];
  for (let run = 0; run < 10; run++) {
    const start = performance.now();
    for (let index = 0; index < 50; index++) {
      const view = toolPresentation(tool);
      assert(view.summary.length <= 321);
      if (tool.status) assert.equal(view.failed, true);
    }
    if (run >= 3) samples.push(performance.now() - start);
  }
  let parses = 0; const parse = JSON.parse;
  try { JSON.parse = (...args) => { parses++; return parse(...args); }; for (let i=0;i<50;i++) toolPresentation(tool); }
  finally { JSON.parse = parse; }
  reports.push({ scenario, cards: 50, argumentCharacters: tool.arguments_json.length, medianMs: median(samples), samplesMs: samples, parses });
}
for (const scenario of ['malformed-unique','path-unique','code-unique']) {
  const tools = Array.from({length:50},(_,index)=>({arguments_json:scenario.startsWith('malformed') ? '{"patch":"'+'x'.repeat(1024*1024)+index
    : JSON.stringify(scenario.startsWith('path') ? {file_path:`/synthetic/${index}.rs`,content:'x'.repeat(1024*1024)} : {code:'x'.repeat(1024*1024)+`//${index}`,language:'python'})}));
  const samples=[];
  for(let run=0;run<10;run++) {
    const start=performance.now();
    for(const tool of tools) assert(toolPresentation(tool).summary.length<=321);
    if(run>=3)samples.push(performance.now()-start);
  }
  let parses=0;const parse=JSON.parse;
  try {JSON.parse=(...args)=>{parses++;return parse(...args);};for(const tool of tools)toolPresentation(tool);}
  finally {JSON.parse=parse;}
  reports.push({scenario,cards:50,medianMs:median(samples),samplesMs:samples,parses});
}
const { parsePrompt } = await import(new URL('transcript.js', source));
const normalPrompt = '<USER_REQUEST>Help with the parser</USER_REQUEST><environment_context>synthetic context</environment_context>';
const normalSamples = [];
for (let run = 0; run < 10; run++) {
  const start = performance.now();
  for (let i = 0; i < 1000; i++) assert.equal(parsePrompt(normalPrompt).text, 'Help with the parser');
  if (run >= 3) normalSamples.push(performance.now() - start);
}
reports.push({scenario:'normal-prompt-control',prompts:1000,medianMs:median(normalSamples),samplesMs:normalSamples});
for (const count of [1000, 4000, 16000]) {
  const text = '<USER_REQUEST>x'.repeat(count) + '</OTHER>';
  const samples = [];
  for (let run = 0; run < 10; run++) {
    const start = performance.now();
    const prompt = parsePrompt(text);
    if (run >= 3) samples.push(performance.now() - start);
    assert.equal(prompt.text, text);
  }
  reports.push({ scenario:'unclosed-prompt-wrappers', openers:count, characters:text.length, medianMs:median(samples), samplesMs:samples });
}
const deep = '['.repeat(12000) + '0' + ']'.repeat(12000);
let formattingError = null, preserved = false;
try { preserved = toolPresentation({ arguments_json: deep }).args === deep; }
catch (error) { formattingError = error.name; }
reports.push({ scenario:'deep-json-formatting', characters:deep.length, formattingError, originalPreserved:preserved });
for (const scenario of ['stuck-worker', 'blocked-startup', 'duplicate-blocks', 'plain-text-fallback', 'cancel-active']) {
  const samples = [], starts = [], posts = [];
  for (let run = 0; run < 6; run++) {
    let started = 0, posted = 0;
    const client = createSyntaxClient(() => {
      started++;
      if (scenario === 'blocked-startup') return Promise.reject(Error('Synthetic startup failure'));
      return {
        timer: null,
        postMessage({ id, text }) {
          posted++;
          if (scenario === 'stuck-worker' || (scenario === 'cancel-active' && text === 'stuck')) return;
          this.timer = setTimeout(() => this.onmessage({ data: { id, nodes: scenario === 'plain-text-fallback' ? null : [text] } }), 2);
        },
        terminate() { clearTimeout(this.timer); },
      };
    }, { timeout: 20, idle: 100 });
    const start = performance.now();
    const request = text => new Promise(resolve => client.request(text, 'js', resolve));
    try {
      if (scenario === 'plain-text-fallback') {
        for (let i = 0; i < 25; i++) assert.equal(await request('const same = 1;'), null);
      } else if (scenario === 'cancel-active') {
        let cancelledDelivered = false;
        const cancel = client.request('stuck', 'js', () => { cancelledDelivered = true; });
        await Promise.resolve(); cancel();
        assert.deepEqual(await request('current'), ['current']); assert.equal(cancelledDelivered, false);
      } else {
        const results = await Promise.all(Array.from({ length: 25 }, (_, i) => request(scenario === 'duplicate-blocks' ? 'const same = 1;' : `const item${i} = 1;`)));
        if (scenario !== 'duplicate-blocks') assert(results.every(result => result === null));
        else assert(results.every(result => result?.[0] === 'const same = 1;'));
      }
      if (run >= 2) { samples.push(performance.now() - start); starts.push(started); posts.push(posted); }
    } finally { client.dispose(); }
  }
  reports.push({ scenario, timeoutMs: 20, blocks: scenario === 'cancel-active' ? 2 : 25, medianMs: median(samples), workerStarts: starts, workerPosts: posts, samplesMs: samples });
}
console.log(JSON.stringify({ source: source.pathname, note: 'Worker timeout scaled to 20 ms for repeatability; production default is 2500 ms.', cases: reports }, null, 2));
