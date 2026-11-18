import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// Optional module path allows comparisons against an archived implementation.
const moduleUrl = process.argv[2] ? pathToFileURL(resolve(process.argv[2])) : new URL('../src/tool-presentation.js', import.meta.url);
const { toolPresentation } = await import(moduleUrl);
for (const size of [1024, 64 * 1024, 1024 * 1024]) {
  const command = '# Synthetic recorded command\nprintf "%s\\n" "' + 'x'.repeat(size) + '"';
  const tool = { tool_name: 'exec_command', command_text: command, arguments_json: JSON.stringify({ cmd: command, workdir: '/synthetic/project', timeout: 30 }), result_text: '{"ok":true}', status: 'completed' };
  const count = size === 1024 * 1024 ? 50 : 200;
  const samples = [];
  let summary;
  for (let run = 0; run < 12; run++) {
    const start = performance.now();
    for (let index = 0; index < count; index++) summary = toolPresentation(tool).summary;
    if (run >= 3) samples.push(performance.now() - start);
    assert.equal(summary, '# Synthetic recorded command');
  }
  samples.sort((a, b) => a - b);
  console.log(JSON.stringify({ scenario: 'collapsed-tool-summary', argumentCharacters: tool.arguments_json.length, cards: count, medianMs: samples[Math.floor(samples.length / 2)], p95Ms: samples[Math.ceil(samples.length * 0.95) - 1], samplesMs: samples }));
}
