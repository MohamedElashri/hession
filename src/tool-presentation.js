import { toolSummary } from './transcript.js';

/** Normalize indexed tool records and grouped conversation calls for display. */
export function toolPresentation(tool) {
  const name = tool.name || tool.tool_name || 'Tool';
  const rawArgs = tool.args ?? tool.arguments_json;
  let parsed = rawArgs;
  try { parsed = JSON.parse(rawArgs); } catch { /* Preserve non-JSON tool input. */ }
  let command = tool.command || tool.command_text || null;
  if (command == null && parsed && typeof parsed === 'object') {
    for (const key of ['command', 'cmd', 'CommandLine']) {
      // Keep array arguments as JSON; joining would lose quoting and boundaries.
      if (typeof parsed[key] === 'string') { command = parsed[key]; break; }
    }
  }
  const code = command == null && typeof parsed?.code === 'string' ? parsed.code : null;
  const args = rawArgs && rawArgs !== '{}' && rawArgs !== 'null'
    ? (typeof parsed === 'object' ? JSON.stringify(parsed, null, 2) : String(parsed)) : null;
  const result = tool.result ?? tool.result_text ?? null;
  const exit = tool.exit_code;
  const failed = ['error', 'failed', 'failure'].includes(String(tool.status ?? '').toLowerCase()) || (exit != null && exit !== 0);
  const outcome = exit != null ? `Exit ${exit}` : failed ? 'Failed' : tool.status || (result != null ? 'Output recorded' : 'Invoked');
  const rawSummary = tool.summary ?? toolSummary({ command_text: command, arguments_json: rawArgs });
  const summary = rawSummary.length > 320 ? `${rawSummary.slice(0, 320)}…` : rawSummary;
  return { name, command, code, args, result, failed, outcome, summary };
}
