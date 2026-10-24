import { test } from 'node:test';
import assert from 'node:assert/strict';
import { textPreview } from './text-preview.js';

test('small messages remain complete, including the boundary', () => {
  for (const value of ['', 'hello', 'x'.repeat(1600), 'line\n'.repeat(27)]) assert.equal(textPreview(value), value);
});

test('large messages have bounded previews without splitting surrogate pairs', () => {
  const text = 'x'.repeat(1599) + '🦀' + 'tail'.repeat(10000);
  assert.equal(textPreview(text), 'x'.repeat(1599));
  assert.equal(textPreview('x'.repeat(50000)).length, 1600);
  assert.equal(textPreview('line\n'.repeat(100)).split('\n').length, 28);
});
