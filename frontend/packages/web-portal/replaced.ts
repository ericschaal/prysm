import { useEffect, useRef, useState } from 'react';

/**
 * Asks whether the script at `url` is gone. A page's script is named by its
 * content, so once Prysm is updated the old page's script is no longer
 * served. Hosts pass their own script's address (`import.meta.url`).
 */
export const scriptReplaced = (url: string) => async () => {
  try {
    const response = await fetch(url, { cache: 'no-store' });
    return response.status === 404;
  } catch {
    return false;
  }
};

/**
 * Whether Prysm was updated under this page: asked each time the connection
 * returns, since an update restarts Prysm. Once true it stays true.
 */
export function useReplaced(
  online: boolean,
  replaced: (() => Promise<boolean>) | undefined,
) {
  const [updated, setUpdated] = useState(false);
  // Lost after having been connected: the first connect is not a return.
  const connectedRef = useRef(false);
  const lostRef = useRef(false);
  useEffect(() => {
    if (!online) {
      lostRef.current = connectedRef.current;
      return;
    }
    connectedRef.current = true;
    if (!lostRef.current || !replaced) return;
    lostRef.current = false;
    let current = true;
    void replaced().then((gone) => {
      if (gone && current) setUpdated(true);
    });
    return () => {
      current = false;
    };
  }, [online, replaced]);
  return updated;
}
