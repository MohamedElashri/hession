import { useEffect, useRef, useState } from 'react';

export default function useDropdown() {
  const [open, setOpen] = useState(false);
  const anchorRef = useRef(null);

  useEffect(() => {
    if (!open) return;
    const outside = event => {
      if (!anchorRef.current?.contains(event.target)) setOpen(false);
    };
    const escape = event => {
      if (event.key !== 'Escape') return;
      setOpen(false);
      anchorRef.current?.querySelector('button')?.focus();
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('keydown', escape);
    };
  }, [open]);

  return { open, setOpen, anchorRef };
}
