import { test } from 'node:test';
import assert from 'node:assert/strict';
import { canHighlight, syntaxLanguage, syntaxTreeSize } from './syntax.js';
import { highlightSyntax } from './syntax-engine.js';

const samples = {
  bash: "printf '%s\\n' \"$HOME\"\n# comment\n",
  powershell: 'Get-Item "$HOME" | Select-Object Name\n',
  javascript: 'const greeting = "🦀 café";\r\nconsole.log(greeting);\n',
  typescript: 'const count: number = 42;\n', python: 'def greet(name):\n\treturn f"Hi {name}"\n',
  json: '{"a": "<script>&🦀", "b": [true, null, 42]}\n', rust: 'fn main() { println!("hello"); }\n',
  diff: '--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n', yaml: 'name: demo\nenabled: true\n',
  xml: '<img src="x" onerror="alert(1)">', css: '.card { color: red; }\n', sql: 'SELECT * FROM sessions WHERE id = 1;\n',
  go: 'package main\nfunc main() { println("hello") }\n', java: 'class Demo { int value = 1; }\n',
  c: '#include <stdio.h>\nint main(void) { return 0; }\n', cpp: '#include <vector>\nstd::vector<int> values;\n', ini: '[demo]\nname = "test"\n',
};
for (const [language, text] of Object.entries(samples)) test(`${language} syntax preserves every source character`, () => {
  const nodes = highlightSyntax(text, language);
  assert.ok(nodes, language); assert.ok(syntaxTreeSize(nodes, text));
  assert.ok(JSON.stringify(nodes).includes('hljs-'), `${language} has tokens`);
});
test('syntax aliases are explicit; unknown languages and unbounded input remain plain', () => {
  assert.equal(syntaxLanguage('TSX'), 'typescript'); assert.equal(syntaxLanguage('ps1'), 'powershell');
  assert.equal(syntaxLanguage('made-up'), 'plaintext');
  for (const name of ['constructor', '__proto__', { toString: null }]) assert.equal(syntaxLanguage(name), 'plaintext');
  for (const text of ['', null, 'a'.repeat(2001), 'x\n'.repeat(13000)]) assert.equal(canHighlight(text, 'bash'), false);
  assert.equal(highlightSyntax('const a = 1;', 'unknown'), null);
  assert.equal(canHighlight('a'.repeat(2000), 'bash'), true);
});
test('token trees reject altered text, executable attributes, excess nodes and deep nesting', () => {
  assert.equal(syntaxTreeSize(['changed'], 'original'), 0);
  assert.equal(syntaxTreeSize([{ className: 'x" onclick="bad', children: ['x'] }], 'x'), 0);
  assert.equal(syntaxTreeSize(Array(6001).fill('x'), 'x'.repeat(6001)), 0);
  let node = 'x'; for (let i = 0; i < 30; i++) node = { className: 'hljs-string', children: [node] };
  assert.equal(syntaxTreeSize([node], 'x'), 0);
});
