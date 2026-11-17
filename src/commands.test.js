import test from 'node:test';
import assert from 'node:assert/strict';
import { createCommandRegistry, isMacPlatform, shortcutLabel } from './commands.js';

function key(key, extra = {}) {
  return { key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false,
    target: { closest: () => null }, preventDefault() { this.defaultPrevented = true; }, ...extra };
}

test('command definitions share execution, availability and platform shortcuts', () => {
  let count = 0;
  const registry = createCommandRegistry([{ id: 'find', title: 'Find', shortcuts: ['Mod+k'],
    disabled: context => context.busy ? 'Busy' : null, run: () => count++ }]);
  assert.equal(registry.handleKeyDown(key('k', { metaKey: true }), { mac: true }), true);
  assert.equal(registry.handleKeyDown(key('k', { ctrlKey: true }), { mac: false }), true);
  assert.equal(registry.handleKeyDown(key('k', { ctrlKey: true }), { mac: true }), false);
  assert.equal(registry.handleKeyDown(key('k', { metaKey: true, ctrlKey: true }), { mac: true }), false);
  assert.equal(registry.execute('find', { busy: true }), false);
  assert.equal(registry.execute('missing'), false);
  assert.equal(registry.execute('find'), true);
  assert.equal(count, 3);
  assert.equal(shortcutLabel('Mod+Shift+k', true), 'Cmd+Shift+K');
  assert.equal(shortcutLabel('Mod+f', false), 'Ctrl+F');
  assert.equal(isMacPlatform('MacIntel'), true);
  assert.equal(isMacPlatform('Linux aarch64'), false);
  assert.throws(() => createCommandRegistry([{ id: 'x' }, { id: 'x' }]), /Duplicate/);
});

test('dispatch respects text editing, composition, repeat, modifiers and modal scope', () => {
  let count = 0;
  const registry = createCommandRegistry([
    { id: 'search', title: 'Search', shortcuts: ['/'], run: () => count++ },
    { id: 'palette', title: 'Palette', shortcuts: ['Mod+k'], allowInInput: true, run: () => count++ },
    { id: 'list', title: 'Next', shortcuts: ['j'], when: context => context.list, run: () => count++ },
    { id: 'help', title: 'Help', shortcuts: ['?'], run: () => count++ },
  ]);
  for (const extra of [{ isComposing: true }, { keyCode: 229 }, { repeat: true }, { defaultPrevented: true }, { altKey: true },
    { target: { closest: () => ({}) } }]) assert.equal(registry.handleKeyDown(key('/', extra)), false);
  assert.equal(registry.handleKeyDown(key('/'), { modalOpen: true }), false);
  assert.equal(registry.handleKeyDown(key('j')), false);
  assert.equal(registry.handleKeyDown(key('j'), { list: true }), true);
  assert.equal(registry.handleKeyDown(key('?', { shiftKey: true })), true);
  assert.equal(registry.handleKeyDown(key('/', { shiftKey: true })), true);
  assert.equal(registry.handleKeyDown(key('k', { ctrlKey: true, target: { closest: () => ({}) } }), { mac: false }), true);
  assert.equal(count, 4);
});
