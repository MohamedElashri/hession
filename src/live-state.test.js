import test from 'node:test';
import assert from 'node:assert/strict';
import { createLiveCommands, liveEntrySummary, liveStatusLabel, MAX_LIVE_BYTES, MAX_LIVE_ENTRIES, mergeLiveEntries } from './live-state.js';

const entry = (id, ordinal = id) => ({ id, ordinal, parts: [{ kind: 'text', text: `Event ${id}` }] });

test('live byte limits include JSON array brackets and separators at the exact boundary', () => {
  const row = { ...entry(1), parts: [{ kind: 'text', text: '' }] };
  const encoder = new TextEncoder();
  const overhead = encoder.encode(JSON.stringify(row)).byteLength;
  row.parts[0].text = 'x'.repeat(MAX_LIVE_BYTES - overhead - 2);
  const exact = mergeLiveEntries([], [row], true);
  assert.equal(exact.entries.length, 1);
  assert.equal(encoder.encode(JSON.stringify(exact.entries)).byteLength, MAX_LIVE_BYTES);
  const larger = { ...row, parts: [{ kind: 'text', text: row.parts[0].text + 'x' }] };
  assert.equal(mergeLiveEntries([], [larger], true).entries.length, 0);
  const extra = entry(2);
  const combined = mergeLiveEntries([], [row, extra], true);
  assert.ok(encoder.encode(JSON.stringify(combined.entries)).byteLength <= MAX_LIVE_BYTES);
});

test('live merge updates stable IDs, preserves gapped order and replaces resets', () => {
  const old = [entry(1, 10), entry(2, 900)];
  const changed = { ...entry(1, 10), tools: [{ result_text: 'Late result' }] };
  const next = mergeLiveEntries(old, [changed, entry(3, 1000)]);
  assert.deepEqual(next.entries.map(e => e.ordinal), [10, 900, 1000]);
  assert.equal(next.entries[0].tools[0].result_text, 'Late result');
  assert.equal(next.added, 1); assert.equal(next.updated, 1);
  assert.deepEqual(mergeLiveEntries(next.entries, [entry(4, 40)], true).entries, [entry(4, 40)]);
  const reused = mergeLiveEntries(next.entries, [{ ...entry(1, 10), parts: [{ kind: 'text', text: 'Rebuilt event' }] }], true);
  assert.equal(reused.updated, 0); assert.equal(reused.added, 1);
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

test('replayed IDs are counted once and their final snapshot wins', () => {
  const original = entry(1);
  const final = { ...entry(2), parts: [{ kind: 'text', text: 'Final output' }] };
  const next = mergeLiveEntries([original], [entry(2), final, { ...original, tools: ['intermediate'] }, structuredClone(original)]);
  assert.equal(next.added, 1); assert.equal(next.updated, 0);
  assert.equal(next.entries[0], original); assert.equal(next.entries[1], final);
  const update = mergeLiveEntries(next.entries, [entry(2), entry(2)]);
  assert.equal(update.updated, 1); assert.equal(update.added, 0);
  assert.throws(() => mergeLiveEntries([], [{ id: 1, ordinal: null }, entry(1)]), /Invalid/);
});
test('identical deltas preserve references while changes replace only the changed record', () => {
  const previous = [entry(1), entry(2)];
  const same = mergeLiveEntries(previous, structuredClone(previous));
  assert.equal(same.entries, previous); assert.equal(same.updated, 0);
  const changed = { ...entry(2), tools: [{ result_text: 'Late output' }] };
  const next = mergeLiveEntries(previous, [structuredClone(previous[0]), changed]);
  assert.equal(next.entries[0], previous[0]); assert.equal(next.entries[1], changed);
  assert.equal(next.updated, 1);
  assert.notEqual(mergeLiveEntries(previous, structuredClone(previous), true).entries, previous);
});
test('size checks stay fresh when callers modify previously supplied objects', () => {
  const previous = mergeLiveEntries([], [entry(1)]).entries;
  previous[0].parts[0].text = '界'.repeat(MAX_LIVE_BYTES);
  const next = mergeLiveEntries(previous, [structuredClone(previous[0])]);
  assert.deepEqual(next.entries, []); assert.equal(next.trimmed, 1);
});
