import { invoke } from '@tauri-apps/api/core';

// Every backend call is a Tauri IPC command (src-tauri/src/lib.rs lists them).
// Rust errors arrive as strings; normalize them to Error.
export async function api(command, args = {}) {
  try {
    return await invoke(command, args);
  } catch (e) {
    const error = e instanceof Error ? e : new Error(typeof e === 'string' ? e : (e?.message || JSON.stringify(e ?? 'Unknown error')));
    // Structured search failures retain actionable cursor and syntax details.
    if (typeof e?.kind === 'string') error.kind = e.kind;
    if (Number.isInteger(e?.position) && e.position >= 0) error.position = e.position;
    throw error;
  }
}

export const hasTauriRuntime = () => typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__?.invoke;
