import { invoke } from '@tauri-apps/api/core';

// The native reading projection sends a repeated tool output once. Restore
// the result part by reference so display/copy retain the full original string.
function restoreReadingOutputs(value) {
  if (!Array.isArray(value?.entries)) return value;
  const outputs = new Map();
  for (const entry of value.entries) for (const tool of entry.tools || []) {
    if (typeof tool.result_text === 'string') outputs.set(String(tool.id), tool.result_text);
  }
  for (const entry of value.entries) for (const part of entry.parts || []) {
    if (part.tool_result_id == null) continue;
    const key = String(part.tool_result_id);
    if (!outputs.has(key)) throw new Error('Tool output changed while loading this conversation. Refresh to retry.');
    part.text = outputs.get(key);
    delete part.tool_result_id;
  }
  return value;
}

// Every backend call is a Tauri IPC command (src-tauri/src/lib.rs lists them).
// Rust errors arrive as strings; normalize them to Error.
export async function api(command, args = {}) {
  try {
    const value = await invoke(command, args);
    return command === 'get_session' ? restoreReadingOutputs(value) : value;
  } catch (e) {
    const error = e instanceof Error ? e : new Error(typeof e === 'string' ? e : (e?.message || JSON.stringify(e ?? 'Unknown error')));
    // Structured search failures retain actionable cursor and syntax details.
    if (typeof e?.kind === 'string') error.kind = e.kind;
    if (Number.isInteger(e?.position) && e.position >= 0) error.position = e.position;
    throw error;
  }
}

export const hasTauriRuntime = () => typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__?.invoke;
