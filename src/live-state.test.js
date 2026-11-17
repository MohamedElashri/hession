import test from 'node:test';
import assert from 'node:assert/strict';
import { createLiveCommands, liveEntrySummary, liveStatusLabel, MAX_LIVE_BYTES, MAX_LIVE_ENTRIES, mergeLiveEntries } from './live-state.js';

const entry = (id, ordinal = id) => ({ id, ordinal, parts: [{ kind: 'text', text: `Event ${id}` }] });

test('live merge updates stable IDs, preserves gapped order and replaces resets', () => {
  const old = [entry(1, 10), entry(2, 900)];
  const changed = { ...entry(1, 10), tools: [{ result_text: 'Late result' }] };
  const next = mergeLiveEntries(old, [changed, entry(3, 1000)]);
  assert.deepEqual(next.entries.map(e => e.ordinal), [10, 900, 1000]);
  assert.equal(next.entries[0].tools[0].result_text, 'Late result');
  assert.equal(next.added, 1); assert.equal(next.updated, 1);
  assert.deepEqual(mergeLiveEntries(next.entries, [entry(4, 40)], true).entries, [entry(4, 40)]);
  const historical = mergeLiveEntries(next.entries, [entry(5, 5)]);
  assert.equal(historical.added, 0); assert.equal(historical.updated, 1);
});

test('live window bounds rows and retained bytes and rejects malformed batches', () => {
  let entries = [];
  for (let page = 0; page < 5; page++) entries = mergeLiveEntries(entries, Array.from({ length: 100 }, (_, n) => entry(page * 100 + n + 1))).entries;
  assert.equal(entries.length, MAX_LIVE_ENTRIES); assert.equal(entries[0].id, 201);
  entries = mergeLiveEntries(entries, [ { ...entry(501), search_text: 'x'.repeat(MAX_LIVE_BYTES / 4) }, { ...entry(502), search_text: '界'.repeat(MAX_LIVE_BYTES / 4) } ]).entries;
  assert.ok(new TextEncoder().encode(JSON.stringify(entries)).byteLength <= MAX_LIVE_BYTES);
  assert.throws(() => mergeLiveEntries([], [{ id: 1, ordinal: null }]), /Invalid/);
  assert.throws(() => mergeLiveEntries([], Array.from({ length: 101 }, (_, n) => entry(n + 1))), /Invalid/);
});

test('live commands reuse supplied observer controls and explain unavailable actions', () => {
  let toggles = 0;
  const observer = { available: true, enabled: true, busy: false, toggle: () => toggles++ };
  const commands = createLiveCommands({ openLive() {}, observer });
  assert.equal(commands.find(c => c.id === 'indexing.pause').disabled(), null);
  assert.match(commands.find(c => c.id === 'indexing.resume').disabled(), /already enabled/);
  commands.find(c => c.id === 'indexing.pause').run(); assert.equal(toggles, 1);
  assert.match(commands.find(c => c.id === 'view.files').disabled(), /Open a project/);
  assert.equal(liveStatusLabel('recent'), 'Recently updated');
  assert.equal(liveStatusLabel('stale'), 'May be stale');
});

test('collapsed live summaries never use combined text or non-conversation text parts', () => {
  for (const [role, kind] of [['assistant', 'reasoning'], ['tool', 'message'], ['system', 'message'], ['assistant', 'unknown_MODEL_EVENT']]) {
    const value = { role, kind, search_text: 'combinedsecret', parts: [{ kind: 'text', text: 'privatesecret' }], tools: [{ tool_name: 'shell', arguments_json: 'argumentsecret', result_text: 'outputsecret' }] };
    assert.equal(liveEntrySummary(value), 'Recorded tool activity: shell');
  }
  assert.equal(liveEntrySummary({ role: 'assistant', kind: 'message', search_text: 'combinedsecret', parts: [{ kind: 'thinking', text: 'privatesecret' }] }), 'Recorded event. Expand to inspect its content.');
  assert.equal(liveEntrySummary({ role: 'assistant', kind: 'agent_message', parts: [{ kind: 'text', text: 'Visible answer' }, { kind: 'thinking', text: 'privatesecret' }] }), 'Visible answer');
});
