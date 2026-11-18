import { toolSummary } from './transcript.js';
import { recordedSyntax, syntaxLanguage } from './syntax.js';
import { textPreview } from './text-preview.js';

// Repeated large payloads occur in tool cards and their transcript replays. Keep
// only display strings, never a parsed JSON tree whose memory can dwarf its
// source. Formatting full arguments remains lazy and local to each view.
const previews = new Map();
const MAX_PREVIEW_CHARACTERS = 4 * 1024 * 1024;
let previewCharacters = 0;
function previewKey(raw) {
  const middle = Math.floor(raw.length / 2);
  // Short keys avoid hashing every byte of a cold, unique megabyte payload.
  // This is only a lookup hint: a hit must also match the complete source.
  return JSON.stringify([raw.length, raw.slice(0, 64), raw.slice(middle, middle + 64), raw.slice(-64)]);
}
function rememberPreview(key, raw, preview) {
  const characters = key.length + raw.length + (preview.command?.length || 0) + (preview.code?.length || 0) + preview.summary.length;
  if (characters > MAX_PREVIEW_CHARACTERS) return;
  const old = previews.get(key);
  if (old) { previewCharacters -= old.characters; previews.delete(key); }
  previews.set(key, { ...preview, raw, characters }); previewCharacters += characters;
  while (previews.size > 4 || previewCharacters > MAX_PREVIEW_CHARACTERS) {
    const key = previews.keys().next().value;
    previewCharacters -= previews.get(key).characters; previews.delete(key);
  }
}
function impossibleLargeJson(raw) {
  if (typeof raw !== 'string' || raw.length < 16384) return false;
  let first = 0, last = raw.length - 1;
  const space = char => char === ' ' || char === '\n' || char === '\r' || char === '\t';
  while (first <= last && space(raw[first])) first++;
  while (last >= first && space(raw[last])) last--;
  const closer = { '{': '}', '[': ']', '"': '"' }[raw[first]];
  return first > last || (closer != null && raw[last] !== closer);
}

/** Normalize indexed tool records and grouped conversation calls for display. */
export function toolPresentation(tool) {
  const name = tool.name || tool.tool_name || 'Tool';
  const rawArgs = tool.args ?? tool.arguments_json;
  const reusable = typeof rawArgs === 'string' && rawArgs.length >= 16384 && rawArgs.length <= 2 * 1024 * 1024
    && !tool.command && !tool.command_text && tool.summary == null;
  const key = reusable ? previewKey(rawArgs) : null;
  const candidate = reusable ? previews.get(key) : null;
  const cached = candidate?.raw === rawArgs ? candidate : null;
  if (cached) { previews.delete(key); previews.set(key, cached); }
  let parsed = rawArgs;
  let json = rawArgs != null && typeof rawArgs === 'object';
  let parsedInput = false;
  const input = () => {
    if (!parsedInput) {
      parsedInput = true;
      if (!cached?.invalid && !impossibleLargeJson(rawArgs)) {
        try { parsed = JSON.parse(rawArgs); json = true; } catch { /* Preserve non-JSON tool input. */ }
      }
    }
    return parsed;
  };
  let command = tool.command || tool.command_text || null;
  // Indexed commands already provide everything needed by the collapsed card.
  // Decode other inputs only when their contents are actually needed.
  if (command == null) { if (cached) command = cached.command; else input(); }
  if (command == null && parsed && typeof parsed === 'object') {
    for (const key of ['command', 'cmd', 'CommandLine']) {
      // Keep array arguments as JSON; joining would lose quoting and boundaries.
      if (typeof parsed[key] === 'string') { command = parsed[key]; break; }
    }
  }
  const code = cached ? cached.code : command == null && typeof parsed?.code === 'string' ? parsed.code : null;
  const hasArguments = Boolean(rawArgs && rawArgs !== '{}' && rawArgs !== 'null');
  let args;
  const result = tool.result ?? tool.result_text ?? null;
  const exit = tool.exit_code;
  const failed = ['error', 'failed', 'failure'].includes(String(tool.status ?? '').toLowerCase()) || (exit != null && exit !== 0);
  const completed = !failed && (exit === 0 || ['completed', 'success', 'ok', 'succeeded'].includes(String(tool.status ?? '').toLowerCase()));
  const outcome = failed ? 'Failed' : completed ? 'Completed' : tool.status ? `Recorded: ${String(tool.status).slice(0, 80)}` : result != null ? 'Output recorded' : 'Invoked';
  const shortName = name.split('__').at(-1).split('.').at(-1).toLowerCase();
  const actions = { exec_command: 'Run command', shell_command: 'Run command', run_command: 'Run command', bash: 'Run command',
    read_file: 'Read file', read: 'Read file', write_file: 'Write file', edit_file: 'Edit file', apply_patch: 'Apply patch' };
  const action = command != null ? 'Run command' : code != null ? 'Run code' : Object.hasOwn(actions, shortName) ? actions[shortName] : name;
  const rawSummary = tool.summary ?? cached?.summary ?? toolSummary({ command_text: command, arguments_json: parsedInput ? parsed : rawArgs }, { argumentsParsed: parsedInput });
  const summary = rawSummary.length > 320 ? `${textPreview(rawSummary, 320, Infinity)}…` : rawSummary;
  if (reusable && !cached) rememberPreview(key, rawArgs, { command, code, summary, invalid: !json });
  return {
    name, action, command, code, hasArguments, result, failed, completed, exit, outcome, summary,
    // Full arguments can be megabytes. Keep their formatting and syntax hints
    // lazy, scoped to this view, and cached once the arguments panel opens.
    get args() {
      if (args === undefined) {
        input();
        try { args = hasArguments ? (json ? JSON.stringify(parsed, null, 2) : String(parsed)) : null; }
        catch { args = typeof rawArgs === 'string' ? rawArgs : String(rawArgs); }
      }
      return args;
    },
    get argsLanguage() { input(); return json ? 'json' : 'plaintext'; },
    get commandLanguage() { return /powershell|pwsh/i.test(name) || syntaxLanguage(input()?.shell) === 'powershell' ? 'powershell' : 'bash'; },
    get codeLanguage() { return syntaxLanguage(input()?.language || parsed?.lang || (/python/i.test(name) ? 'python' : /javascript|node_repl|(?:^|[._])js$|^functions\.exec$/i.test(name) ? 'javascript' : 'plaintext')); },
    get resultLanguage() { return recordedSyntax(result); },
  };
}
