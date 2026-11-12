import { useSyncExternalStore } from 'react';
import { Settings } from 'lucide-react';
import { updates } from '../updates.js';

// Reading update state never checks the network. Keep progress and the restart
// action discoverable even after Settings has been closed.
export default function SettingsButton({ onClick }) {
  const { phase } = useSyncExternalStore(updates.subscribe, updates.getSnapshot);
  const label = {
    checking: 'Checking for updates…',
    available: 'Update available',
    downloading: 'Downloading update…',
    verifying: 'Verifying update…',
    installing: 'Installing update…',
    ready: 'Update ready to restart',
    restarting: 'Restarting…',
  }[phase];
  return <button type="button" className="nav-item settings-entry" aria-label="Settings"
    onClick={() => onClick(label ? 'about' : undefined)} aria-haspopup="dialog">
    <Settings size={16}/><span>Settings{label && <small role="status">{label}</small>}</span>
  </button>;
}
