import assert from 'node:assert/strict';
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = fileURLToPath(new URL('../../', import.meta.url));
const files = ['README.md', ...readdirSync(resolve(root, 'docs')).filter(name => name.endsWith('.md')).map(name => `docs/${name}`)];
const read = file => readFileSync(resolve(root, file), 'utf8');
const withoutFences = source => source.replace(/^(```|~~~)[^\n]*\n[\s\S]*?^\1[^\n]*$/gm, '');
const slug = text => text.toLowerCase().replace(/[^\p{L}\p{N}_\-\s]/gu, '').replace(/ /g, '-');

test('canonical documentation links, anchors, and README images exist', () => {
  for (const file of files) {
    const source = withoutFences(read(file));
    const links = [...source.matchAll(/\]\(([^\s)]+)(?:\s+"[^"]*")?\)/g)].map(match => match[1]);
    links.push(...[...source.matchAll(/(?:src|srcset)="([^"]+)"/g)].map(match => match[1]));
    for (const href of links) {
      if (/^[a-z][\w+.-]*:/i.test(href)) continue;
      const [path, anchor] = href.split('#');
      const target = path ? resolve(root, dirname(file), decodeURIComponent(path)) : resolve(root, file);
      assert(existsSync(target), `${file}: missing ${href}`);
      if (!anchor) continue;
      const headings = [...withoutFences(readFileSync(target, 'utf8')).matchAll(/^#{1,6} (.+)$/gm)].map(match => slug(match[1]));
      assert(headings.includes(decodeURIComponent(anchor)), `${file}: missing anchor ${href}`);
    }
  }
});

test('README describes current features without hardcoded release numbers', () => {
  assert.doesNotMatch(read('README.md'), /\bv?\d+\.\d+\.\d+\b/);
  assert.doesNotMatch(read('README.md'), /^## New in /m);
});
