import { test } from 'node:test';
import assert from 'node:assert/strict';
import { customTerminalArguments, TERMINAL_ARGUMENTS } from './terminal-settings.js';

test('terminal arguments preserve spaces, quotes, and shell metacharacters literally', () => {
  assert.deepEqual(customTerminalArguments('--title\nMy "coding" session; $HOME\r\nsh\r\n{script}\r\n'),
    ['--title', 'My "coding" session; $HOME', 'sh', '{script}']);
});

test('every launch convention includes exactly one script argument', () => {
  for (const args of Object.values(TERMINAL_ARGUMENTS)) assert.deepEqual(customTerminalArguments(args.join('\n')), args);
  for (const text of ['', '-e sh {script}', '"{script}"', '{script}\n{script}']) {
    assert.throws(() => customTerminalArguments(text), /exactly once/);
  }
});
