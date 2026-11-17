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
});
test('recorded code languages are honored without guessing a language for generic code', () => {
  assert.equal(toolPresentation({ name: 'functions.exec', args: '{"code":"const a = 1;"}' }).codeLanguage, 'javascript');
  assert.equal(toolPresentation({ name: 'python', args: '{"code":"print(1)"}' }).codeLanguage, 'python');
  assert.equal(toolPresentation({ name: 'custom', args: '{"code":"x", "language":"rs"}' }).codeLanguage, 'rust');
  assert.equal(toolPresentation({ name: 'custom', args: '{"code":"x"}' }).codeLanguage, 'plaintext');
  assert.equal(toolPresentation({ name: 'powershell', command_text: 'Get-Item .'}).commandLanguage, 'powershell');
});
test('only structured output gets an automatic syntax hint and valid JSON arguments keep their representation', () => {
  assert.equal(toolPresentation({ result_text: '{"ok":true}' }).resultLanguage, 'json');
  assert.equal(toolPresentation({ result_text: 'INFO const value = 1;' }).resultLanguage, 'plaintext');
  assert.equal(toolPresentation({ result_text: '--- a/file\n+++ b/file\n-old\n+new' }).resultLanguage, 'diff');
  assert.equal(toolPresentation({ arguments_json: '"a string"' }).args, '"a string"');
});
