/**
 * A small QR code encoder for the calibration address (ISO/IEC 18004): byte
 * mode, error correction level M, versions 1–6 (up to 106 bytes, which needs
 * no version information), and the mask with the lowest penalty.
 */

/** Level M per version 1–6: error correction codewords per block, blocks, data codewords per block. */
const blocksM: readonly (readonly [number, number, number])[] = [
  [10, 1, 16],
  [16, 1, 28],
  [26, 1, 44],
  [18, 2, 32],
  [24, 2, 43],
  [16, 4, 27],
];

/** Multiplication in GF(256) with the QR polynomial 0x11d. */
function multiply(x: number, y: number): number {
  let z = 0;
  for (let i = 7; i >= 0; i--) {
    z = (z << 1) ^ ((z >>> 7) * 0x11d);
    z ^= ((y >>> i) & 1) * x;
  }
  return z;
}

/** The Reed–Solomon error correction codewords of `data`. */
function errorCorrection(data: readonly number[], degree: number): number[] {
  const divisor: number[] = [];
  for (let i = 0; i < degree; i++) divisor.push(i === degree - 1 ? 1 : 0);
  let root = 1;
  for (let i = 0; i < degree; i++) {
    for (let j = 0; j < degree; j++) {
      divisor[j] = multiply(divisor[j]!, root) ^ (j + 1 < degree ? divisor[j + 1]! : 0);
    }
    root = multiply(root, 2);
  }
  const result: number[] = divisor.map(() => 0);
  for (const byte of data) {
    const factor = byte ^ result.shift()!;
    result.push(0);
    for (let i = 0; i < degree; i++) result[i] = result[i]! ^ multiply(divisor[i]!, factor);
  }
  return result;
}

const masks: readonly ((x: number, y: number) => boolean)[] = [
  (x, y) => (x + y) % 2 === 0,
  (_, y) => y % 2 === 0,
  (x) => x % 3 === 0,
  (x, y) => (x + y) % 3 === 0,
  (x, y) => (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0,
  (x, y) => ((x * y) % 2) + ((x * y) % 3) === 0,
  (x, y) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
  (x, y) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
];

/** The standard penalty of a symbol (lower scans better). */
function penalty(modules: readonly boolean[][]): number {
  const size = modules.length;
  const lines: string[] = [];
  for (let i = 0; i < size; i++) {
    let row = '';
    let column = '';
    for (let j = 0; j < size; j++) {
      row += modules[i]![j] ? '1' : '0';
      column += modules[j]![i] ? '1' : '0';
    }
    lines.push(row, column);
  }
  let score = 0;
  for (const line of lines) {
    for (const run of line.match(/0{5,}|1{5,}/g) ?? []) score += run.length - 2;
    score += 40 * (line.match(/(?=10111010000|00001011101)/g) ?? []).length;
  }
  let dark = 0;
  for (let y = 0; y < size; y++)
    for (let x = 0; x < size; x++) {
      if (modules[y]![x]) dark++;
      const here = modules[y]![x];
      if (
        x + 1 < size &&
        y + 1 < size &&
        modules[y]![x + 1] === here &&
        modules[y + 1]![x] === here &&
        modules[y + 1]![x + 1] === here
      )
        score += 3;
    }
  const total = size * size;
  return score + 10 * (Math.ceil(Math.abs(dark * 20 - total * 10) / total) - 1);
}

/**
 * The modules of `text` as rows (`true` is dark), without the quiet zone, or
 * `null` when it does not fit version 6. `mask` forces a mask (for tests).
 */
export function qrCode(text: string, mask?: number): boolean[][] | null {
  const bytes = Array.from(new TextEncoder().encode(text));
  const version = blocksM.findIndex(([, blocks, data]) => bytes.length + 2 <= blocks * data) + 1;
  if (!version) return null;
  const [ecPerBlock, blocks, dataPerBlock] = blocksM[version - 1]!;

  // Mode, length, data, terminator, then pad bytes.
  const bits: number[] = [];
  const put = (value: number, length: number) => {
    for (let i = length - 1; i >= 0; i--) bits.push((value >>> i) & 1);
  };
  put(0b0100, 4);
  put(bytes.length, 8);
  for (const byte of bytes) put(byte, 8);
  const capacity = blocks * dataPerBlock;
  put(0, Math.min(4, capacity * 8 - bits.length));
  while (bits.length % 8) bits.push(0);
  const data: number[] = [];
  for (let i = 0; i < bits.length; i += 8) {
    let byte = 0;
    for (let j = 0; j < 8; j++) byte = (byte << 1) | bits[i + j]!;
    data.push(byte);
  }
  for (let pad = 0xec; data.length < capacity; pad ^= 0xec ^ 0x11) data.push(pad);

  // Split into blocks and interleave the data, then the error correction.
  const dataBlocks: number[][] = [];
  const ecBlocks: number[][] = [];
  for (let block = 0; block < blocks; block++) {
    const part = data.slice(block * dataPerBlock, (block + 1) * dataPerBlock);
    dataBlocks.push(part);
    ecBlocks.push(errorCorrection(part, ecPerBlock));
  }
  const codewords: number[] = [];
  for (let i = 0; i < dataPerBlock; i++) for (const part of dataBlocks) codewords.push(part[i]!);
  for (let i = 0; i < ecPerBlock; i++) for (const part of ecBlocks) codewords.push(part[i]!);

  const size = 17 + 4 * version;
  const grid = () => Array.from({ length: size }, () => Array.from({ length: size }, () => false));
  const modules = grid();
  const reserved = grid();
  const set = (x: number, y: number, dark: boolean) => {
    modules[y]![x] = dark;
    reserved[y]![x] = true;
  };
  for (let i = 0; i < size; i++) {
    set(6, i, i % 2 === 0);
    set(i, 6, i % 2 === 0);
  }
  for (const [cx, cy] of [
    [3, 3],
    [size - 4, 3],
    [3, size - 4],
  ] as const)
    for (let dy = -4; dy <= 4; dy++)
      for (let dx = -4; dx <= 4; dx++) {
        const x = cx + dx;
        const y = cy + dy;
        const ring = Math.max(Math.abs(dx), Math.abs(dy));
        if (x >= 0 && x < size && y >= 0 && y < size) set(x, y, ring !== 2 && ring !== 4);
      }
  if (version > 1)
    for (let dy = -2; dy <= 2; dy++)
      for (let dx = -2; dx <= 2; dx++)
        set(size - 7 + dx, size - 7 + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
  /** Format information of level M (00) and `chosen` into `target`, twice. */
  const drawFormat = (target: boolean[][], chosen: number) => {
    let remainder = chosen;
    for (let i = 0; i < 10; i++) remainder = (remainder << 1) ^ ((remainder >>> 9) * 0x537);
    const format = ((chosen << 10) | remainder) ^ 0x5412;
    const put = (x: number, y: number, i: number) => {
      target[y]![x] = ((format >>> i) & 1) === 1;
      reserved[y]![x] = true;
    };
    for (let i = 0; i <= 5; i++) put(8, i, i);
    put(8, 7, 6);
    put(8, 8, 7);
    put(7, 8, 8);
    for (let i = 9; i < 15; i++) put(14 - i, 8, i);
    for (let i = 0; i < 8; i++) put(size - 1 - i, 8, i);
    for (let i = 8; i < 15; i++) put(8, size - 15 + i, i);
    set(8, size - 8, true);
  };
  drawFormat(modules, 0);

  // Data in two-module columns, zigzagging up and down from the bottom right.
  let index = 0;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    const upward = ((right + 1) & 2) === 0;
    for (let step = 0; step < size; step++)
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const y = upward ? size - 1 - step : step;
        if (reserved[y]![x] || index >= codewords.length * 8) continue;
        modules[y]![x] = ((codewords[index >>> 3]! >>> (7 - (index & 7))) & 1) === 1;
        index++;
      }
  }

  const masked = (chosen: number) => {
    const result = modules.map((row, y) =>
      row.map((dark, x) => (reserved[y]![x] ? dark : dark !== masks[chosen]!(x, y))),
    );
    drawFormat(result, chosen);
    return result;
  };
  if (mask !== undefined) return masked(mask);
  let best: boolean[][] = [];
  let bestScore = Infinity;
  for (let chosen = 0; chosen < 8; chosen++) {
    const candidate = masked(chosen);
    const score = penalty(candidate);
    if (score < bestScore) {
      best = candidate;
      bestScore = score;
    }
  }
  return best;
}
