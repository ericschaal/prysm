import { useEffect, useRef, useState } from 'react';

/**
 * Asks whether the script at `url` is gone. A page's script is named by its
 * content, so once Prysm is updated the old page's script is no longer
 * served. Hosts pass their own script's address (`import.meta.url`).
 */
export const scriptReplaced = (url: string) => async () => {
  try {
    const response = await fetch(url, { method: 'HEAD', cache: 'no-store' });
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
  // The first connect is not a return: only a connection lost and back is.
  const connection = useRef<'first' | 'connected' | 'lost'>('first');
  useEffect(() => {
    if (!online) {
      if (connection.current === 'connected') connection.current = 'lost';
      return;
    }
    const returned = connection.current === 'lost';
    connection.current = 'connected';
    if (!returned || !replaced) return;
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
