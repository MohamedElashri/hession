export const SYNTAX_LANGUAGES = [
  ['plaintext', 'Plain text'], ['bash', 'Shell'], ['powershell', 'PowerShell'],
  ['javascript', 'JavaScript'], ['typescript', 'TypeScript'], ['python', 'Python'],
  ['json', 'JSON'], ['rust', 'Rust'], ['diff', 'Diff'], ['yaml', 'YAML'],
  ['xml', 'HTML/XML'], ['css', 'CSS'], ['sql', 'SQL'], ['go', 'Go'],
  ['java', 'Java'], ['c', 'C'], ['cpp', 'C++'], ['ini', 'TOML/INI'],
];
const languages = new Set(SYNTAX_LANGUAGES.map(([id]) => id));
const aliases = { text: 'plaintext', plain: 'plaintext', txt: 'plaintext', sh: 'bash', shell: 'bash', zsh: 'bash',
  js: 'javascript', jsx: 'javascript', ts: 'typescript', tsx: 'typescript', py: 'python', rs: 'rust', patch: 'diff',
  yml: 'yaml', html: 'xml', htm: 'xml', xml: 'xml', ps1: 'powershell', pwsh: 'powershell', toml: 'ini', 'c++': 'cpp' };

export function syntaxLanguage(value) {
  const name = String(value ?? '').toLowerCase();
  return aliases[name] || (languages.has(name) ? name : 'plaintext');
}

// Keep both work and retained results bounded. Larger text remains fully readable/copyable.
const MAX_SYNTAX_CHARACTERS = 24000;
/** Recognize structured data, without guessing a language from prose or logs. */
export function recordedSyntax(text) {
  if (typeof text !== 'string' || text.length > MAX_SYNTAX_CHARACTERS) return 'plaintext';
  if (/^\s*[[{]/.test(text)) { try { JSON.parse(text); return 'json'; } catch { /* Not a JSON document. */ } }
  return /^diff --git |^--- [^\n]*\n\+\+\+ /m.test(text) ? 'diff' : 'plaintext';
}
export function canHighlight(text, language) {
  if (typeof text !== 'string' || !text || text.length > MAX_SYNTAX_CHARACTERS || syntaxLanguage(language) === 'plaintext') return false;
  let start = 0;
  for (let i = 0; i <= text.length; i++) {
    if (i === text.length || text[i] === '\n') {
      if (i - start > 2000) return false;
      start = i + 1;
    }
  }
  return true;
}

/** Only a bounded tree of text and token spans may cross the rendering boundary. */
export function syntaxTreeSize(nodes, original) {
  if (!Array.isArray(nodes) || nodes.length > 6000) return 0;
  const stack = nodes.map(node => [node, 0]).reverse(), text = [];
  let count = 0;
  while (stack.length) {
    const [node, depth] = stack.pop();
    if (++count > 6000 || depth > 24) return 0;
    if (typeof node === 'string') text.push(node);
    else if (node && typeof node.className === 'string' && node.className.length <= 200 && /^(?:[a-zA-Z_][\w-]* ?)*$/.test(node.className) && Array.isArray(node.children)) {
      if (stack.length + node.children.length > 6000) return 0;
      for (let i = node.children.length - 1; i >= 0; i--) stack.push([node.children[i], depth + 1]);
    } else return 0;
  }
  return text.join('') === original ? count : 0;
}
