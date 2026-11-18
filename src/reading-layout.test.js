import { test } from 'node:test';
import assert from 'node:assert/strict';
import { applyReadingWidths, createReadingLayout, READING_DEFAULTS } from './reading-layout.js';

test('reading preferences clamp corrupt widths and never reopen trapped in focus mode', () => {
  const storage = { getItem: () => JSON.stringify({ navWidth: 10000, listWidth: -2, readingWidth: 'wide', navHidden: true, focusMode: true, searchVisible: true }), setItem() {} };
  const layout = createReadingLayout(storage);
  assert.deepEqual(layout.getSnapshot(), { ...READING_DEFAULTS, navWidth: 360, listWidth: 220, navHidden: true });
  let saved;
  const persistent = createReadingLayout({ getItem() {}, setItem: (_, value) => { saved = JSON.parse(value); } });
  persistent.update({ focusMode: true, searchVisible: true, listWidth: 420 });
  assert.equal(saved.listWidth, 420); assert(!('focusMode' in saved)); assert(!('searchVisible' in saved));
});

test('dragging changes CSS widths without invalidating transcript visibility subscribers or persisting each move', () => {
  let writes = 0;
  const layout = createReadingLayout({ getItem() {}, setItem() { writes++; } });
  const visibility = layout.getVisibility();
  const properties = new Map();
  const element = { style: { setProperty: (name, value) => properties.set(name, value) } };
  const stop = layout.subscribe(() => applyReadingWidths(element, layout.getSnapshot()));
  for (let width = 200; width <= 350; width++) layout.update({ navWidth: width }, { persist: false });
  assert.equal(layout.getVisibility(), visibility); assert.equal(writes, 0);
  assert.equal(properties.get('--sidebar-w'), '350px');
  layout.save(); assert.equal(writes, 1);
  layout.update({ fullWidth: true }); assert.equal(properties.get('--reading-w'), 'none');
  layout.update({ focusMode: true }); assert.notEqual(layout.getVisibility(), visibility);
  layout.update({ focusMode: false }); assert.equal(layout.getSnapshot().navWidth, 350);
  layout.reset(); assert.deepEqual(layout.getSnapshot(), READING_DEFAULTS);
  stop();
});

test('unavailable preference storage preserves a usable in-memory layout', () => {
  const layout = createReadingLayout({ getItem() { throw Error('blocked'); }, setItem() { throw Error('quota'); } });
  layout.update({ listHidden: true }); assert.equal(layout.getSnapshot().listHidden, true);
  assert.match(layout.getSaveError(), /Could not save/);
});
