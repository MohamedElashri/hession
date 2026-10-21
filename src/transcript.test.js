// Run with `node --test src/` (part of `make test`).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildConversation, parsePrompt, toolSummary } from './transcript.js';

const text = (id, role, body, extra = {}) => ({ id, role, parts: [{ id: `p${id}`, kind: 'text', text: body }], ...extra });

test('typed text is separated from injected context', () => {
  const p = parsePrompt('<system-reminder>be careful</system-reminder>\nFix the bug in <b>main</b>');
  assert.equal(p.text, 'Fix the bug in <b>main</b>');
  assert.deepEqual(p.context.map(c => c.label), ['System reminder']);
});

test('Antigravity USER_REQUEST is the prompt; metadata is context', () => {
  const p = parsePrompt('<USER_REQUEST>Why is it slow?</USER_REQUEST><ADDITIONAL_METADATA>time</ADDITIONAL_METADATA>');
  assert.equal(p.text, 'Why is it slow?');
  assert.deepEqual(p.context.map(c => c.tag), ['ADDITIONAL_METADATA']);
});

test('slash commands become a command, not text', () => {
  const p = parsePrompt('<command-name>/review</command-name><command-message>review</command-message><command-args>PR 12</command-args>');
  assert.equal(p.command, '/review PR 12');
  assert.equal(p.text, '');
});

test('turns group replies and tools; results attach to their call; bookkeeping is dropped', () => {
  const turns = buildConversation([
    { id: 1, role: 'mode', parts: [{ kind: 'event' }] },
    text(2, 'user', 'Run the tests'),
    { id: 3, role: 'assistant', parts: [{ kind: 'thinking', text: 'plan' }, { kind: 'text', text: 'Running.' }], tools: [{ id: 9, call_id: 'c1', tool_name: 'Bash', command_text: 'cargo test' }] },
    { id: 4, role: 'user', parts: [{ kind: 'tool_result', text: 'ok', data_json: '{"tool_use_id":"c1"}' }] },
    text(5, 'user', '<system-reminder>x</system-reminder>'),
    { id: 6, role: 'assistant', kind: 'api-request-blob', parts: [{ kind: 'text', text: 'side request' }] },
    text(7, 'assistant', 'All green.'),
    text(8, 'user', 'Thanks'),
  ]);
  assert.equal(turns.length, 2);
  assert.equal(turns[0].prompt.text, 'Run the tests');
  assert.deepEqual(turns[0].items.map(i => i.type), ['thinking', 'text', 'tools', 'context', 'text']);
  assert.equal(turns[0].items[2].calls[0].result, 'ok');
  assert.equal(turns[0].items[2].calls[0].summary, 'cargo test');
  assert.equal(turns[1].prompt.text, 'Thanks');
});

test('Antigravity GENERIC output attaches to the open tool call', () => {
  const turns = buildConversation([
    text(1, 'user', '<USER_REQUEST>check load</USER_REQUEST>'),
    { id: 2, role: 'assistant', parts: [], tools: [{ id: 5, tool_name: 'run_command', arguments_json: '{"CommandLine":"uptime"}' }] },
    { id: 3, role: 'assistant', kind: 'GENERIC', parts: [{ kind: 'text', text: 'Created At: now\nup 1 day' }] },
  ]);
  const call = turns[0].items[0].calls[0];
  assert.equal(call.summary, 'uptime');
  assert.match(call.result, /up 1 day/);
});

test('a prompt that is only pasted text is still a prompt', () => {
  const [turn] = buildConversation([text(1, 'user', '<pasted_content id="a">log line 1\nline 2</pasted_content>')]);
  assert.deepEqual(turn.prompt.pasted, ['log line 1\nline 2']);
  assert.equal(turn.prompt.text, '');
});

test('placeholders typed in angle brackets stay in the prompt', () => {
  assert.equal(parsePrompt('rename <slug> to <name>').text, 'rename <slug> to <name>');
});

test('tool arguments fall back to the tool-call part', () => {
  const [turn] = buildConversation([
    text(1, 'user', 'go'),
    { id: 2, role: 'assistant', parts: [{ kind: 'tool_call', data_json: '{"args":{"CommandLine":"uptime"}}' }], tools: [{ id: 3, tool_name: 'run_command', arguments_json: '{}' }] },
  ]);
  assert.equal(turn.items[0].calls[0].summary, 'uptime');
});

test('context sent before the first prompt joins that prompt', () => {
  const turns = buildConversation([
    text(1, 'user', '<environment_context>cwd=/x</environment_context>'),
    text(2, 'user', 'Start'),
  ]);
  assert.equal(turns.length, 1);
  assert.deepEqual(turns[0].prompt.context.map(c => c.label), ['Environment']);
});

test('tool summaries prefer commands, then paths', () => {
  assert.equal(toolSummary({ arguments_json: '{"command":["bash","-lc","ls"]}' }), 'bash -lc ls');
  assert.equal(toolSummary({ arguments_json: '{"file_path":"src/a.rs"}' }), 'src/a.rs');
  assert.equal(toolSummary({ arguments_json: '*** Begin Patch\nfoo' }), '*** Begin Patch');
});
