import { invoke } from '@tauri-apps/api/core';

// Every backend call is a Tauri IPC command (src-tauri/src/lib.rs lists them).
// Rust errors arrive as strings; normalize them to Error.
export async function api(command, args = {}) {
  try {
    return await invoke(command, args);
  } catch (e) {
    throw e instanceof Error ? e : new Error(typeof e === 'string' ? e : (e?.message || JSON.stringify(e ?? 'Unknown error')));
  }
}

export const hasTauriRuntime = () => typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__?.invoke;
