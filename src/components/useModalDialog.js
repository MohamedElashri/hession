import { useLayoutEffect, useRef } from 'react';

export default function useModalDialog() {
  const dialog = useRef(null);
  useLayoutEffect(() => {
    const element = dialog.current;
    const previous = document.activeElement;
    element.showModal();
    return () => {
      // Release the native modal before React removes it and restores focus.
      element.close();
      if (previous?.isConnected) previous.focus();
    };
  }, []);
  return dialog;
}
