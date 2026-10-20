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
