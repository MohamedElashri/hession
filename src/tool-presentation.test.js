import { test } from 'node:test';
import assert from 'node:assert/strict';
import { toolPresentation } from './tool-presentation.js';
import { buildConversation } from './transcript.js';

test('command presentation preserves exact shell text and all auxiliary arguments', () => {
  const command = "printf '%s\\n' 'a b'\ncat <<'EOF'\n$HOME `literal`\nEOF";
  const view = toolPresentation({ tool_name: 'exec', arguments_json: JSON.stringify({ cmd: command, workdir: '/a b', timeout: 30 }), result_text: '' });
  assert.equal(view.command, command);
  assert.equal(JSON.parse(view.args).workdir, '/a b');
  assert.equal(view.result, '');
  assert.equal(view.outcome, 'Output recorded');
});

test('argument arrays are displayed without inventing shell quoting', () => {
  const args = { command: ['bash', '-lc', 'echo "two words"'], cwd: '/tmp' };
  const view = toolPresentation({ arguments_json: JSON.stringify(args) });
  assert.equal(view.command, null);
  assert.deepEqual(JSON.parse(view.args), args);
});

test('missing output stays passive, malformed arguments remain visible, and summary is bounded', () => {
  const view = toolPresentation({ arguments_json: 'not-json', command_text: 'x'.repeat(30000) });
  assert.equal(view.result, null);
  assert.equal(view.outcome, 'Invoked');
  assert.equal(view.args, 'not-json');
  assert.ok(view.summary.length < 330);
  assert.equal(view.command.length, 30000);
});

test('exit failures survive conversation grouping and take priority over a success status', () => {
  const [turn] = buildConversation([{ role: 'assistant', tools: [{ id: 1, status: 'completed', exit_code: 2, result_text: 'failure' }] }]);
  const view = toolPresentation(turn.items[0].calls[0]);
  assert.equal(view.failed, true);
  assert.equal(view.outcome, 'Failed');
  assert.equal(view.exit, 2);
  assert.equal(toolPresentation({ exit_code: 0 }).failed, false);
  assert.equal(toolPresentation({ status: 'error' }).failed, true);
});

test('readable actions retain tool identity and distinguish failures even with exit zero', () => {
  const view = toolPresentation({ tool_name: 'mcp__files__read_file', status: 'failed', exit_code: 0 });
  assert.equal(view.action, 'Read file'); assert.equal(view.name, 'mcp__files__read_file');
  assert.equal(view.outcome, 'Failed'); assert.equal(view.completed, false);
  assert.equal(toolPresentation({ exit_code: 0 }).outcome, 'Completed');
  assert.equal(toolPresentation({ status: 'running' }).outcome, 'Recorded: running');
  assert.equal(toolPresentation({ tool_name: 'constructor' }).action, 'constructor');
  assert.equal(toolPresentation({ tool_name: '__proto__' }).action, '__proto__');
});
test('recorded code languages are honored without guessing a language for generic code', () => {
  assert.equal(toolPresentation({ name: 'functions.exec', args: '{"code":"const a = 1;"}' }).codeLanguage, 'javascript');
  assert.equal(toolPresentation({ name: 'python', args: '{"code":"print(1)"}' }).codeLanguage, 'python');
  assert.equal(toolPresentation({ name: 'custom', args: '{"code":"x", "language":"rs"}' }).codeLanguage, 'rust');
  assert.equal(toolPresentation({ name: 'custom', args: '{"code":"x"}' }).codeLanguage, 'plaintext');
  assert.equal(toolPresentation({ name: 'custom', args: '{"code":"x", "language":{"toString":null}}' }).codeLanguage, 'plaintext');
  assert.equal(toolPresentation({ name: 'powershell', command_text: 'Get-Item .'}).commandLanguage, 'powershell');
});
test('only structured output gets an automatic syntax hint and valid JSON arguments keep their representation', () => {
  assert.equal(toolPresentation({ result_text: '{"ok":true}' }).resultLanguage, 'json');
  assert.equal(toolPresentation({ result_text: 'INFO const value = 1;' }).resultLanguage, 'plaintext');
  assert.equal(toolPresentation({ result_text: '--- a/file\n+++ b/file\n-old\n+new' }).resultLanguage, 'diff');
  assert.equal(toolPresentation({ arguments_json: '"a string"' }).args, '"a string"');
});

test('collapsed indexed commands defer argument serialization and cache it on demand', () => {
  let serialized = 0;
  const command = 'echo "recorded only"';
  const input = { toJSON() { serialized++; return { cmd: command, workdir: '/synthetic' }; } };
  const view = toolPresentation({ tool_name: 'exec_command', command_text: command, arguments_json: input });
  assert.equal(view.summary, command);
  assert.equal(view.action, 'Run command');
  assert.equal(view.hasArguments, true);
  assert.equal(serialized, 0);
  const formatted = view.args;
  assert.deepEqual(JSON.parse(formatted), { cmd: command, workdir: '/synthetic' });
  assert.equal(view.args, formatted);
  assert.equal(serialized, 1);
});

test('fallback summaries decode arguments once and retain JSON string literals', t => {
  const parse = JSON.parse; let calls = 0;
  t.mock.method(JSON, 'parse', (...args) => { calls++; return parse(...args); });
  for (const [arguments_json, summary] of [
    ['{"file_path":"/synthetic/a.rs"}', '/synthetic/a.rs'],
    ['{"code":"print(1)","language":"python"}', 'print(1)'],
    ['{"invalid":"unterminated', '{"invalid":"unterminated'],
    ['"{\\"cmd\\":\\"literal JSON string\\"}"', '{"cmd":"literal JSON string"}'],
  ]) {
    calls = 0;
    const view = toolPresentation({ arguments_json, status: 'failed' });
    assert.equal(view.summary, summary); assert.equal(calls, 1);
    assert.equal(view.outcome, 'Failed');
    void view.args; void view.argsLanguage;
    assert.equal(calls, 1, 'expanding arguments reuses the existing decode');
  }
});

test('deep valid JSON remains readable when pretty-printing exceeds the JS stack', () => {
  const raw = '['.repeat(12000) + '0' + ']'.repeat(12000);
  const view = toolPresentation({ arguments_json: raw });
  assert.equal(view.args, raw); assert.equal(view.argsLanguage, 'json');
  assert.equal(view.args, raw);
});

test('large repeated payloads reuse previews while full arguments and per-call outcomes stay accurate', t => {
  const raw = JSON.stringify({ code: 'print("界")\n'.repeat(2500), language: 'python', private_field: 'preserved' });
  const parse = JSON.parse; let calls = 0;
  t.mock.method(JSON, 'parse', (...args) => { calls++; return parse(...args); });
  const first = toolPresentation({ name: 'python', arguments_json: raw, status: 'failed' });
  assert.equal(calls, 1);
  const replay = toolPresentation({ name: 'functions.exec', arguments_json: raw, status: 'completed' });
  assert.equal(calls, 1, 'a replay does not decode the large tree to render a collapsed card');
  assert.equal(replay.code, first.code); assert.equal(replay.summary, first.summary);
  assert.equal(replay.outcome, 'Completed'); assert.equal(first.outcome, 'Failed');
  assert.deepEqual(parse(replay.args), parse(raw)); assert.equal(replay.codeLanguage, 'python');
  assert.equal(calls, 2, 'expansion decodes once, then reuses the view-local result');
  void replay.args; assert.equal(calls, 2);
});

test('large malformed envelopes avoid a full JSON parse and preserve the recorded input', t => {
  const raw = ' \n{"patch":"' + '界'.repeat(100000) + '\t ';
  const parse = JSON.parse; let calls = 0;
  t.mock.method(JSON, 'parse', (...args) => { calls++; return parse(...args); });
  const view = toolPresentation({ arguments_json: raw });
  assert.equal(calls, 0); assert.equal(view.args, raw); assert.equal(view.argsLanguage, 'plaintext');
  assert(view.summary.length <= 321);
});

test('large-preview retention is bounded and changes to argument strings invalidate previews', t => {
  const raw = index => JSON.stringify({ file_path: `/fixture/${index}.rs`, content: 'x'.repeat(20000) });
  const parse = JSON.parse; let calls = 0;
  t.mock.method(JSON, 'parse', (...args) => { calls++; return parse(...args); });
  for (let i = 0; i < 8; i++) assert.equal(toolPresentation({ arguments_json: raw(i) }).summary, `/fixture/${i}.rs`);
  const before = calls;
  assert.equal(toolPresentation({ arguments_json: raw(0) }).summary, '/fixture/0.rs');
  assert.equal(calls, before + 1, 'older previews are evicted instead of retaining every payload');
});

test('preview lookup hints never reuse content when complete argument strings differ', () => {
  const code = 'x'.repeat(100000);
  const changed = code.slice(0, 1000) + 'y' + code.slice(1001);
  const first = toolPresentation({ arguments_json: JSON.stringify({ code, language: 'python' }) });
  const second = toolPresentation({ arguments_json: JSON.stringify({ code: changed, language: 'python' }) });
  assert.equal(first.code, code); assert.equal(second.code, changed);
  assert.equal(JSON.parse(second.args).code, changed);
});
