/** Keep Tab inside the active dialog while preserving modified browser shortcuts. */
export function trapDialogFocus(event, dialog) {
  if (event.key !== 'Tab' || event.altKey || event.ctrlKey || event.metaKey || !dialog) return false;
  const controls = Array.from(dialog.querySelectorAll('button,input,select,textarea,a[href],[tabindex]:not([tabindex="-1"])'))
    .filter(element => !element.disabled && !element.closest('[hidden],[inert]') && element.tabIndex >= 0);
  const first = controls[0], last = controls.at(-1);
  const active = document.activeElement;
  if (!controls.length || !controls.includes(active) || (event.shiftKey ? active === first : active === last)) {
    event.preventDefault();
    (event.shiftKey ? last : first)?.focus();
    return true;
  }
  return false;
}
