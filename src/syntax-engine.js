import { createLowlight } from 'lowlight';
import bash from 'highlight.js/lib/languages/bash';
import powershell from 'highlight.js/lib/languages/powershell';
import javascript from 'highlight.js/lib/languages/javascript';
import typescript from 'highlight.js/lib/languages/typescript';
import python from 'highlight.js/lib/languages/python';
import json from 'highlight.js/lib/languages/json';
import rust from 'highlight.js/lib/languages/rust';
import diff from 'highlight.js/lib/languages/diff';
import yaml from 'highlight.js/lib/languages/yaml';
import xml from 'highlight.js/lib/languages/xml';
import css from 'highlight.js/lib/languages/css';
import sql from 'highlight.js/lib/languages/sql';
import go from 'highlight.js/lib/languages/go';
import java from 'highlight.js/lib/languages/java';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import ini from 'highlight.js/lib/languages/ini';
import { canHighlight, syntaxLanguage, syntaxTreeSize } from './syntax.js';

const lowlight = createLowlight({ bash, powershell, javascript, typescript, python, json, rust, diff, yaml, xml, css, sql, go, java, c, cpp, ini });
function tokens(children, budget = { remaining: 6000 }, depth = 0) {
  if (depth > 24) throw new Error('Token nesting limit');
  return children.map(node => {
    if (--budget.remaining < 0) throw new Error('Token count limit');
    if (node.type === 'text') return node.value;
    if (node.type !== 'element' || node.tagName !== 'span') throw new Error('Unexpected token');
    return { className: (node.properties.className || []).join(' '), children: tokens(node.children, budget, depth + 1) };
  });
}

export function highlightSyntax(text, language) {
  if (!canHighlight(text, language)) return null;
  try {
    const tree = tokens(lowlight.highlight(syntaxLanguage(language), text).children);
    return syntaxTreeSize(tree, text) ? tree : null;
  } catch { return null; }
}
