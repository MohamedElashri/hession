import { X } from 'lucide-react';
import { shortcutLabel } from '../commands.js';
import useModalDialog from './useModalDialog.js';
import { trapDialogFocus } from './dialog-focus.js';

export default function ShortcutHelp({ registry, context, onClose }) {
  const dialog = useModalDialog();
  return <dialog ref={dialog} className="data-dialog shortcut-help" aria-labelledby="shortcut-title"
    onCancel={event => { event.preventDefault(); onClose(); }} onKeyDown={event => {
      if (event.nativeEvent?.isComposing || event.keyCode === 229) return;
      if (trapDialogFocus(event, dialog.current)) return;
      if (event.key === 'Escape' && !event.repeat && !event.altKey && !event.ctrlKey && !event.metaKey) { event.preventDefault(); event.stopPropagation(); onClose(); }
    }}>
    <div className="command-palette-heading"><h2 id="shortcut-title">Keyboard shortcuts</h2><button type="button" autoFocus className="icon-button" aria-label="Close keyboard shortcuts" onClick={onClose}><X size={18}/></button></div>
    <p>Use {context.mac ? 'Command (Cmd)' : 'Control (Ctrl)'} for app shortcuts. Letter shortcuts are paused while typing or using a dialog.</p>
    <table><thead><tr><th>Action</th><th>Shortcut</th><th>Context</th></tr></thead><tbody>
      {registry.commands.filter(command => command.shortcuts?.length).map(command => <tr key={command.id}><td>{command.title}</td><td>{command.shortcuts.map(key => <kbd key={key}>{shortcutLabel(key, context.mac)}</kbd>)}</td><td>{command.scope || 'App'}{command.disabled?.(context) && <small>{command.disabled(context)}</small>}</td></tr>)}
    </tbody></table>
    <p>Tab moves between controls. In the sidebar, ↑/↓ and Home/End move between navigation buttons. Focus the transcript to scroll with arrows, Page Up/Down, Home/End. Escape dismisses menus and dialogs when their current operation allows it.</p>
    <button type="button" className="btn" onClick={onClose}>Done</button>
  </dialog>;
}
