import { test } from 'node:test';
import assert from 'node:assert/strict';
import { applyAppearance, DEFAULT_APPEARANCE, normalizeAppearance, readAppearance } from './appearance.js';
import { storageKey } from './brand.js';

const storageWith = entries => ({ getItem: key => entries[key] ?? null });

test('preserves an existing light or dark preference when no appearance settings exist', () => {
  for (const theme of ['light', 'dark']) {
    assert.deepEqual(readAppearance(storageWith({ [storageKey('theme')]: theme })), { ...DEFAULT_APPEARANCE, theme });
  }
  assert.deepEqual(readAppearance(storageWith({})), DEFAULT_APPEARANCE);
});

test('saved appearance takes precedence over the legacy theme', () => {
  const saved = { theme: 'sepia', accent: 'violet', density: 'compact', textSize: 'large', wrapCode: true, reduceMotion: true };
  assert.deepEqual(readAppearance(storageWith({
    [storageKey('theme')]: 'dark', [storageKey('appearance')]: JSON.stringify(saved),
  })), saved);
});

test('invalid and partial preferences fall back independently', () => {
  assert.deepEqual(normalizeAppearance({ theme: 'midnight', accent: 'invalid', textSize: 90, density: 'tight', wrapCode: 'true', reduceMotion: false }), {
    ...DEFAULT_APPEARANCE, theme: 'midnight',
  });
  for (const value of [null, false, 'dark', [], 42]) assert.deepEqual(normalizeAppearance(value), DEFAULT_APPEARANCE);
});

test('corrupt or inaccessible storage does not prevent startup', () => {
  assert.deepEqual(readAppearance(storageWith({ [storageKey('appearance')]: '{' })), DEFAULT_APPEARANCE);
  assert.deepEqual(readAppearance({ getItem() { throw new Error('Storage blocked'); } }), DEFAULT_APPEARANCE);
  Object.defineProperty(globalThis, 'window', { configurable: true, get() { throw new Error('Storage access blocked'); } });
  try { assert.deepEqual(readAppearance(), DEFAULT_APPEARANCE); }
  finally { delete globalThis.window; }
});

test('system theme follows device changes while explicit themes stay fixed', () => {
  const root = { dataset: {} };
  applyAppearance(DEFAULT_APPEARANCE, true, root);
  assert.equal(root.dataset.theme, 'dark');
  applyAppearance(DEFAULT_APPEARANCE, false, root);
  assert.equal(root.dataset.theme, 'light');
  for (const theme of ['light', 'dark', 'sepia', 'midnight']) {
    applyAppearance({ ...DEFAULT_APPEARANCE, theme }, true, root);
    assert.equal(root.dataset.theme, theme);
    applyAppearance({ ...DEFAULT_APPEARANCE, theme }, false, root);
    assert.equal(root.dataset.theme, theme);
  }
});

test('reset clears every applied override', () => {
  const root = { dataset: {} };
  applyAppearance({ theme: 'midnight', accent: 'rose', textSize: 'large', density: 'compact', wrapCode: true, reduceMotion: true }, true, root);
  applyAppearance(DEFAULT_APPEARANCE, false, root);
  assert.deepEqual(root.dataset, { theme: 'light', accent: 'green', textSize: 'default', density: 'comfortable', wrapCode: 'false', reduceMotion: 'false' });
});
