import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// Pass a saved transcript.js path to compare an earlier implementation.
const moduleUrl = process.argv[2] ? pathToFileURL(resolve(process.argv[2])) : new URL('../src/transcript.js', import.meta.url);
const { buildConversation } = await import(moduleUrl);

for (const toolCount of [100, 1000, 4000]) {
  const entries = [{ id: 1, role: 'user', parts: [{ kind: 'text', text: 'Run tools' }] }];
  const perEntry = 20;
  for (let index = 0; index < toolCount; index += perEntry) {
    entries.push({ id: entries.length + 1, role: 'assistant', parts: [], tools: Array.from({ length: Math.min(perEntry, toolCount - index) }, (_, offset) => ({
      id: index + offset, call_id: `call-${index + offset}`, tool_name: 'Read', arguments_json: '{}',
    })) });
  }
  for (let index = 0; index < toolCount; index += perEntry) {
    entries.push({ id: entries.length + 1, role: 'tool', parts: Array.from({ length: Math.min(perEntry, toolCount - index) }, (_, offset) => ({
      kind: 'tool_result', data_json: JSON.stringify({ call_id: `call-${index + offset}` }), text: 'done',
    })) });
  }
  const timings = [];
  for (let run = 0; run < 10; run++) {
    const start = performance.now();
    const [turn] = buildConversation(entries);
    const duration = performance.now() - start;
    assert.equal(turn.items[0].calls.length, toolCount);
    assert(turn.items[0].calls.every(call => call.result === 'done'));
    if (run >= 3) timings.push(duration);
  }
  timings.sort((a, b) => a - b);
  console.log(JSON.stringify({ entries: entries.length, toolCalls: toolCount, medianMs: Number(timings[3].toFixed(3)) }));
}
