const agentNames = {
  codex: 'Codex', claude: 'Claude Code', opencode: 'OpenCode', pi: 'Pi',
  commandcode: 'Command Code', muse: 'Muse', hermes: 'Hermes Agent',
  antigravity: 'Antigravity', cline: 'Cline',
};

export const agentColors = {
  codex: 'lime', claude: 'peach', opencode: 'blue', pi: 'violet',
  commandcode: 'mint', muse: 'pink', hermes: 'amber', antigravity: 'blue', cline: 'mint',
};

export const familyLabel = family => agentNames[family] || family || 'Agent';
export const shortPath = path => String(path ?? '').replace(/\\/g, '/').split('/').filter(Boolean).at(-1) || 'Local workspace';
