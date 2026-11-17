import { toolSummary } from './transcript.js';
import { recordedSyntax, syntaxLanguage } from './syntax.js';
import { textPreview } from './text-preview.js';

/** Normalize indexed tool records and grouped conversation calls for display. */
export function toolPresentation(tool) {
  const name = tool.name || tool.tool_name || 'Tool';
  const rawArgs = tool.args ?? tool.arguments_json;
  let parsed = rawArgs;
  let json = rawArgs != null && typeof rawArgs === 'object';
  try { parsed = JSON.parse(rawArgs); json = true; } catch { /* Preserve non-JSON tool input. */ }
  let command = tool.command || tool.command_text || null;
  if (command == null && parsed && typeof parsed === 'object') {
    for (const key of ['command', 'cmd', 'CommandLine']) {
      // Keep array arguments as JSON; joining would lose quoting and boundaries.
      if (typeof parsed[key] === 'string') { command = parsed[key]; break; }
    }
  }
  const code = command == null && typeof parsed?.code === 'string' ? parsed.code : null;
  const args = rawArgs && rawArgs !== '{}' && rawArgs !== 'null'
    ? (json ? JSON.stringify(parsed, null, 2) : String(parsed)) : null;
  const result = tool.result ?? tool.result_text ?? null;
  const exit = tool.exit_code;
  const failed = ['error', 'failed', 'failure'].includes(String(tool.status ?? '').toLowerCase()) || (exit != null && exit !== 0);
  const completed = !failed && (exit === 0 || ['completed', 'success', 'ok', 'succeeded'].includes(String(tool.status ?? '').toLowerCase()));
  const outcome = failed ? 'Failed' : completed ? 'Completed' : tool.status ? `Recorded: ${String(tool.status).slice(0, 80)}` : result != null ? 'Output recorded' : 'Invoked';
  const shortName = name.split('__').at(-1).split('.').at(-1).toLowerCase();
  const actions = { exec_command: 'Run command', shell_command: 'Run command', run_command: 'Run command', bash: 'Run command',
    read_file: 'Read file', read: 'Read file', write_file: 'Write file', edit_file: 'Edit file', apply_patch: 'Apply patch' };
  const action = command != null ? 'Run command' : code != null ? 'Run code' : actions[shortName] || name;
  const codeLanguage = syntaxLanguage(parsed?.language || parsed?.lang || (/python/i.test(name) ? 'python' : /javascript|node_repl|(?:^|[._])js$|^functions\.exec$/i.test(name) ? 'javascript' : 'plaintext'));
  const commandLanguage = /powershell|pwsh/i.test(name) || syntaxLanguage(parsed?.shell) === 'powershell' ? 'powershell' : 'bash';
  const resultLanguage = recordedSyntax(result);
  const rawSummary = tool.summary ?? toolSummary({ command_text: command, arguments_json: rawArgs });
  const summary = rawSummary.length > 320 ? `${textPreview(rawSummary, 320, Infinity)}…` : rawSummary;
  return { name, action, command, commandLanguage, code, codeLanguage, args, argsLanguage: json ? 'json' : 'plaintext', result, resultLanguage, failed, completed, exit, outcome, summary };
}
