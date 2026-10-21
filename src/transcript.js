export function hasToolActivity(entry) {
  return entry.role === 'tool' || Boolean(entry.tools?.length) ||
    Boolean(entry.parts?.some(part => part.kind === 'tool_call' || part.kind === 'tool_result'));
}

export function isSessionEvent(entry) {
  return entry.role !== 'user' && entry.role !== 'assistant' && !hasToolActivity(entry);
}

export function transcriptParts(entry, showTools = true) {
  const tools = showTools ? entry.tools || [] : [];
  const parts = entry.parts || [];
  return {
    tools,
    text: parts.filter(p => p.kind === 'text' && p.text && (showTools || entry.role !== 'tool')),
    special: parts.filter(p => {
      if (p.kind === 'text') return false;
      if (p.kind === 'tool_call') return showTools && !tools.length;
      if (p.kind === 'tool_result') return showTools;
      return true;
    }),
  };
}

// ---------------------------------------------------------------------------
// Conversation view: the dialogue grouped into turns, without bookkeeping.
// ---------------------------------------------------------------------------

// Tags whose content is the prompt the user typed (Antigravity).
const PROMPT_TAGS = new Set(['USER_REQUEST']);
const COMMAND_TAGS = new Set(['command-name', 'command-message', 'command-args']);
// Pasted blocks are the user's own content, shown collapsed within the prompt.
const PASTE_TAGS = new Set(['pasted_content']);
const CONTEXT_LABELS = {
  'system-reminder': 'System reminder',
  environment_context: 'Environment',
  user_instructions: 'Instructions',
  'local-command-stdout': 'Command output',
  'local-command-stderr': 'Command error',
  'local-command-caveat': 'Note',
  'fork-boilerplate': 'Fork context',
  ADDITIONAL_METADATA: 'Metadata',
  USER_SETTINGS_CHANGE: 'Settings change',
};
const BLOCK = /<([A-Za-z][\w-]*)(?:\s[^>]*)?>([\s\S]*?)<\/\1>/g;

// Injected wrappers use custom names (dashes, underscores, or capitals); plain
// HTML such as <div> stays part of the text.
const isCustomTag = tag => /[-_A-Z]/.test(tag);

/**
 * Split a user-role text into what was typed and what tools injected.
 * Returns { text, command, pasted: [string], context: [{ tag, label, body }] }.
 */
export function parsePrompt(raw) {
  const typed = [];
  const pasted = [];
  const context = [];
  const command = {};
  const rest = String(raw ?? '').replace(BLOCK, (match, tag, body) => {
    if (PROMPT_TAGS.has(tag)) { typed.push(body.trim()); return ''; }
    if (PASTE_TAGS.has(tag)) { pasted.push(body.trim()); return ''; }
    if (COMMAND_TAGS.has(tag)) { command[tag] = body.trim(); return ''; }
    if (!isCustomTag(tag)) return match;
    context.push({ tag, label: CONTEXT_LABELS[tag] || tag.replaceAll(/[-_]/g, ' ').toLowerCase(), body: body.trim() });
    return '';
  }).trim();
  if (rest) typed.push(rest);
  const name = command['command-name'];
  return {
    text: typed.join('\n\n'),
    command: name ? `${name.startsWith('/') ? '' : '/'}${name}${command['command-args'] ? ` ${command['command-args']}` : ''}` : null,
    pasted,
    context,
  };
}

const partData = part => { try { return JSON.parse(part.data_json || '{}'); } catch { return {}; } };
const resultId = part => { const d = partData(part); return d.tool_use_id || d.call_id || null; };

/** One-line description of a tool call: its command, path, or first argument. */
export function toolSummary(tool) {
  if (tool.command_text) return tool.command_text.split('\n')[0];
  let args = tool.arguments_json;
  try { args = JSON.parse(args); } catch { /* plain-text input */ }
  if (typeof args === 'string') return args.split('\n')[0];
  if (args && typeof args === 'object') {
    for (const key of ['command', 'cmd', 'CommandLine', 'file_path', 'path', 'AbsolutePath', 'Path', 'pattern', 'query', 'url',
      'toolSummary', 'description', 'title', 'prompt', 'Prompt', 'skill', 'Action', 'message', 'input', 'code']) {
      const value = args[key];
      if (Array.isArray(value)) return value.join(' ');
      if (typeof value === 'string' && value.trim()) return value.split('\n')[0];
    }
  }
  return '';
}

/**
 * Group transcript entries into turns: a prompt you typed followed by the
 * agent's replies, thinking, and tool calls (with their results). Bookkeeping
 * (events, attachments, mode changes, logged side requests) is left out; the
 * full log shows it.
 */
export function buildConversation(entries) {
  const turns = [];
  let turn = null;
  const startTurn = prompt => { turn = { id: prompt?.entry.id ?? `start-${turns.length}`, prompt, items: [] }; turns.push(turn); };
  const current = () => { if (!turn) startTurn(null); return turn; };
  const lastItem = () => current().items.at(-1);
  const toolGroup = () => {
    const last = lastItem();
    if (last?.type === 'tools') return last;
    const group = { type: 'tools', key: `tools-${current().items.length}`, calls: [] };
    current().items.push(group);
    return group;
  };
  const findCall = id => turn?.items.flatMap(i => (i.type === 'tools' ? i.calls : [])).find(c => id && c.id === id);
  const addResult = (id, text) => {
    const call = findCall(id) ?? [...(turn?.items ?? [])].reverse().find(i => i.type === 'tools')?.calls.findLast(c => !c.result);
    if (call) { if (!call.result) call.result = text; return; }
    toolGroup().calls.push({ key: `result-${id ?? toolGroup().calls.length}`, name: 'Output', summary: '', result: text });
  };

  for (const entry of entries || []) {
    if (entry.kind === 'api-request-blob') continue;
    const parts = entry.parts || [];
    if (entry.role === 'user') {
      for (const part of parts.filter(p => p.kind === 'tool_result')) addResult(resultId(part), part.text);
      for (const part of parts.filter(p => p.kind === 'text' && p.text)) {
        const prompt = parsePrompt(part.text);
        if (prompt.text || prompt.command || prompt.pasted.length) startTurn({ entry, ...prompt });
        else if (prompt.context.length) current().items.push({ type: 'context', key: `context-${entry.id}`, context: prompt.context });
      }
    } else if (entry.role === 'assistant') {
      // Antigravity reports tool output as GENERIC assistant text.
      if (entry.kind === 'GENERIC') {
        for (const part of parts.filter(p => p.text)) addResult(null, part.text);
        continue;
      }
      for (const part of parts) {
        if (part.kind === 'text' && part.text?.trim()) current().items.push({ type: 'text', key: `text-${part.id ?? entry.id}`, text: part.text, entry });
        else if (part.kind === 'thinking' && part.text?.trim()) current().items.push({ type: 'thinking', key: `thinking-${part.id ?? entry.id}`, text: part.text });
      }
      // Some agents (Antigravity) keep the arguments on the tool-call part.
      const callParts = parts.filter(p => p.kind === 'tool_call');
      (entry.tools || []).forEach((tool, index) => {
        const own = ['', '{}', 'null'].includes(tool.arguments_json ?? '') ? null : tool.arguments_json;
        const fromPart = partData(callParts[index] ?? {}).args;
        const args = own ?? (fromPart == null ? null : JSON.stringify(fromPart));
        const call = { ...tool, arguments_json: args };
        toolGroup().calls.push({ key: `call-${tool.id}`, id: tool.call_id, name: tool.tool_name || 'Tool', summary: toolSummary(call), status: tool.status, command: tool.command_text, args, result: tool.result_text || null });
      });
    } else if (entry.role === 'tool') {
      for (const part of parts.filter(p => p.kind === 'tool_result')) addResult(resultId(part), part.text);
    }
  }
  // Context that arrives before a prompt (Codex sends its environment first)
  // belongs to that prompt rather than to an empty turn of its own.
  const merged = [];
  let pending = [];
  for (const t of turns) {
    if (!t.prompt && t.items.every(i => i.type === 'context')) { pending.push(...t.items.flatMap(i => i.context)); continue; }
    if (pending.length && t.prompt) t.prompt.context = [...pending, ...t.prompt.context];
    else if (pending.length) t.items.unshift({ type: 'context', key: `context-before-${t.id}`, context: pending });
    pending = [];
    merged.push(t);
  }
  if (pending.length) merged.push({ id: 'context-end', prompt: null, items: [{ type: 'context', key: 'context-end', context: pending }] });
  return merged.filter(t => t.prompt || t.items.length);
}
