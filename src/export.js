import { BRAND } from './brand.js';
const escape = value => String(value ?? '').replace(/[\\`*_{}\[\]<>#|]/g, '\\$&').replace(/[\r\n]+/g, ' ');
const pretty = value => { try { return JSON.stringify(JSON.parse(value), null, 2); } catch { return String(value ?? ''); } };
const code = (value, language = 'text') => {
  const text = String(value ?? '');
  const runs = text.match(/`+/g) || [];
  const fence = '`'.repeat(Math.max(3, ...runs.map(run => run.length + 1)));
  return `${fence}${language}\n${text}\n${fence}`;
};
const toolMarkdown = tool => [
  `**${escape(tool.tool_name || 'Tool')}**${tool.status ? ` · ${escape(tool.status)}` : ''}`,
  tool.command_text && code(tool.command_text, 'sh'),
  tool.arguments_json && code(pretty(tool.arguments_json), 'json'),
  tool.result_text && code(tool.result_text),
].filter(Boolean).join('\n\n');
export function libraryAsMarkdown(data) {
  const sections = [`# ${BRAND.name} export`, `Exported: ${escape(data.exportedAt)}`, `${data.sessions.length} session(s)`];
  for (const { session, entries, stats, unlinkedTools } of data.sessions) {
    sections.push(`---\n\n## ${escape(session.title || 'Untitled session')}`);
    sections.push([
      `- Agent: ${escape(session.product || session.agent)}`,
      `- Workspace: ${escape(session.project || session.cwd || 'Unknown')}`,
      `- Started: ${escape(session.started_at || 'Unknown')}`,
      `- Last activity: ${escape(session.last_event_at || 'Unknown')}`,
      `- Starred: ${session.starred ? 'Yes' : 'No'}`,
      `- Tags: ${(session.tags || []).map(escape).join(', ') || 'None'}`,
      session.branch && `- Branch: ${escape(session.branch)}`,
      session.model && `- Model: ${escape(session.model)}`,
    ].filter(Boolean).join('\n'));
    if (stats) sections.push('### Session stats', Object.entries(stats).map(([key, value]) => `- ${escape(key.replaceAll('_', ' '))}: ${escape(value)}`).join('\n'));
    sections.push('### Transcript');
    for (const entry of entries) {
      sections.push(`#### ${escape(entry.role || entry.kind || 'Event')}${entry.occurred_at ? ` · ${escape(entry.occurred_at)}` : ''}`);
      const parts = (entry.parts || []).map(part => {
        if (part.kind === 'text') return part.text || '';
        return [`**${escape(part.kind || 'Event')}**`, part.text && code(part.text), part.data_json && code(pretty(part.data_json), 'json')].filter(Boolean).join('\n\n');
      }).filter(Boolean);
      parts.push(...(entry.tools || []).map(toolMarkdown));
      if (!parts.length) parts.push(entry.search_text || (entry.payload_json && code(pretty(entry.payload_json), 'json')) || `_${escape(entry.kind || 'Event')}_`);
      sections.push(parts.join('\n\n'));
    }
    if (unlinkedTools?.length) sections.push('### Additional tool activity', ...unlinkedTools.map(toolMarkdown));
  }
  return sections.join('\n\n') + '\n';
}

export function exportFilename(scope, session, count, format, now = new Date()) {
  const safe = value => String(value || '').replace(/[^a-zA-Z0-9_-]+/g, '-').replace(/^-+|-+$/g, '') || 'session';
  const pad = number => String(number).padStart(2, '0');
  const localTime = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}_${pad(now.getHours())}-${pad(now.getMinutes())}-${pad(now.getSeconds())}`;
  const minutes = -now.getTimezoneOffset();
  const zone = `UTC${minutes >= 0 ? '+' : '-'}${pad(Math.floor(Math.abs(minutes) / 60))}-${pad(Math.abs(minutes) % 60)}`;
  const name = scope === 'session' ? `${safe(session?.agent || session?.product)}-${safe(session?.external_id || session?.id)}` : scope === 'filtered' ? `filtered-${count}` : 'all';
  return `${BRAND.slug}-${name}_${localTime}_${zone}.${format === 'json' ? 'json' : 'md'}`;
}
