/**
 * `host[:port]` from what a user typed or pasted: surrounding spaces, a leading
 * `http://` and a trailing `/` are dropped. Everything else, including
 * `https://`, is left for the server to accept or explain.
 */
export function normalizeWledAddress(text: string): string {
  const trimmed = text.trim();
  const bare = /^http:\/\//i.test(trimmed)
    ? trimmed.slice('http://'.length)
    : trimmed;
  return bare.endsWith('/') ? bare.slice(0, -1) : bare;
}

/**
 * WLED's own web interface for a saved `address` (`host`, `host:port`,
 * `[ipv6]:port` or a bare IPv6 address), or `null` when it is not one.
 */
export function wledUrl(address: string): string | null {
  const text = normalizeWledAddress(address);
  if (!text || /[\s/?#@\\]/.test(text)) return null;
  // A bare IPv6 address has more than one colon and needs brackets in a URL.
  const authority =
    !text.startsWith('[') && text.indexOf(':') !== text.lastIndexOf(':')
      ? `[${text}]`
      : text;
  try {
    return new URL(`http://${authority}/`).href;
  } catch {
    return null;
  }
}
