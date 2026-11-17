/**
 * @typedef {Object} DesktopCommand
 * @property {string} id Stable, unique identifier.
 * @property {string} title Human-readable action.
 * @property {string} [scope] Availability hint, also shown in shortcut help.
 * @property {string[]} [shortcuts] Platform-neutral keys, e.g. Mod+k.
 * @property {(context: Object) => boolean} [when] Keyboard context predicate.
 * @property {(context: Object) => string|null} [disabled] Reason the action is unavailable.
 * @property {boolean} [allowInInput] Whether a modified shortcut works while typing.
 * @property {boolean} [hidden] Hide context-only navigation from the palette.
 * @property {(context: Object) => void} run Action shared by palette and shortcuts.
 */

export function isMacPlatform(platform = globalThis.navigator?.userAgentData?.platform || globalThis.navigator?.platform || '') {
  return /mac|iphone|ipad/i.test(platform);
}

export function shortcutLabel(shortcut, mac = isMacPlatform()) {
  return shortcut.split('+').map(key => ({ Mod: mac ? 'Cmd' : 'Ctrl', Shift: 'Shift', Alt: 'Alt',
    ArrowDown: '↓', ArrowUp: '↑', Escape: 'Esc', ' ': 'Space' })[key] || (key.length === 1 ? key.toUpperCase() : key)).join('+');
}

/** Restore focus after React has rendered a navigation destination. */
export function focusAfterRender(...selectors) {
  return requestAnimationFrame(() => selectors.map(selector => document.querySelector(selector)).find(Boolean)?.focus());
}

/**
 * Register feature navigation without coupling keyboard code to its screen or IPC.
 * @param {Array<Omit<DesktopCommand, 'run'> & {open: (context: Object) => void, focus?: string}>} destinations
 * @returns {DesktopCommand[]}
 */
export function createNavigationCommands(destinations) {
  return destinations.map(({ open, focus = '.main-panel', ...command }) => ({
    ...command, run: context => { open(context); focusAfterRender(focus); },
  }));
}

// Feature lists use the data attributes; older session cards remain supported.
export const SESSION_ITEM_SELECTOR = '[data-session-navigation],.session-card';
const SESSION_LIST_SELECTOR = '[data-session-list],.session-list';

export function keyboardEventContext(event, context = {}) {
  return { ...context, target: event.target,
    modalOpen: !!context.modalOpen || !!document.querySelector('dialog[open],[aria-modal="true"]'),
    sessionList: event.target.closest?.(SESSION_LIST_SELECTOR),
    onSessionCard: !!event.target.matches?.(SESSION_ITEM_SELECTOR),
  };
}

function isEditingTarget(target) {
  return !!target?.closest?.('input,textarea,select,[contenteditable]:not([contenteditable="false"]),[role="textbox"],[role="combobox"],[role="slider"],[role="spinbutton"],[role="menu"],[role="listbox"],[role="tablist"]');
}

function matchesShortcut(event, shortcut, mac) {
  const keys = shortcut.split('+');
  const key = keys.at(-1);
  const mod = keys.includes('Mod');
  // Punctuation may require Shift on different layouts; compare the produced key.
  return event.key.toLowerCase() === key.toLowerCase()
    && event.metaKey === (mac && mod) && event.ctrlKey === (!mac && mod)
    && event.altKey === keys.includes('Alt')
    && (['?', '/'].includes(key) || event.shiftKey === keys.includes('Shift'));
}

/**
 * One registry supplies dispatch, palette actions and help.
 * @param {DesktopCommand[]} commands Extend with feature command objects.
 */
export function createCommandRegistry(commands) {
  const ids = new Set();
  for (const command of commands) {
    if (!command.id || ids.has(command.id)) throw new Error(`Duplicate or missing command ID: ${command.id}`);
    ids.add(command.id);
  }
  return {
    commands,
    execute(id, context = {}) {
      const command = commands.find(item => item.id === id);
      if (!command || command.disabled?.(context)) return false;
      command.run(context);
      return true;
    },
    handleKeyDown(event, context = {}) {
      if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || event.repeat || context.modalOpen) return false;
      const mac = context.mac ?? isMacPlatform();
      const command = commands.find(item => item.shortcuts?.some(key => matchesShortcut(event, key, mac))
        && (!isEditingTarget(event.target) || item.allowInInput)
        && (!item.when || item.when(context)) && !item.disabled?.(context));
      if (!command) return false;
      event.preventDefault();
      command.run(context);
      return true;
    },
  };
}

/** Move focus without selecting/opening. Native Enter or the list.open command opens the item. */
export function moveRegionFocus(region, selector, direction) {
  const items = Array.from(region?.querySelectorAll(selector) || []).filter(item => !item.disabled);
  if (!items.length) return;
  const current = items.indexOf(document.activeElement);
  const next = direction === 'first' ? 0 : direction === 'last' ? items.length - 1
    : current < 0 ? (direction < 0 ? items.length - 1 : 0) : Math.max(0, Math.min(items.length - 1, current + direction));
  items[next].focus();
  items[next].scrollIntoView?.({ block: 'nearest', behavior: 'instant' });
}
