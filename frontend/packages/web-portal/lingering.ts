import { useEffect, useRef, useState } from 'react';

/** `active`, but only after `delay`, and then for at least `hold`. */
export function useLingering(active: boolean, delay: number, hold: number) {
  const [shown, setShown] = useState(false);
  const shownAtRef = useRef(0);
  useEffect(() => {
    if (active === shown) return;
    const wait = active
      ? delay
      : Math.max(0, hold - (Date.now() - shownAtRef.current));
    const timer = setTimeout(() => {
      if (active) shownAtRef.current = Date.now();
      setShown(active);
    }, wait);
    return () => clearTimeout(timer);
  }, [active, shown, delay, hold]);
  return shown;
}
