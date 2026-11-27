import { create } from 'qrcode';

/** Byte-mode QR at level M, through version 6, without the quiet zone. */
export function qrCode(text: string): boolean[][] | null {
  const data = new TextEncoder().encode(text);
  if (data.length > 106) return null;

  const { modules } = create([{ mode: 'byte', data }], {
    errorCorrectionLevel: 'M',
  });
  return Array.from({ length: modules.size }, (_, y) =>
    Array.from({ length: modules.size }, (_, x) => Boolean(modules.get(y, x))),
  );
}
