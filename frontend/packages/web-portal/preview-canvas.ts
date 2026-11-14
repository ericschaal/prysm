import type { PreviewFrame } from '@prysm/application-client';

/** Canvas pixels around the picture: a colour strip, then a gap. */
const STRIP = 12;
const GAP = 4;
const BORDER = STRIP + GAP;

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}
type Rgb = readonly [number, number, number];

/**
 * Where everything goes on the canvas: the picture inside a border, each
 * edge's colours spread evenly along it in screen orientation (top and bottom
 * left to right, left and right top to bottom, as the stream sends them), and
 * the sampled area when black bars were removed.
 */
export function previewLayout(frame: PreviewFrame) {
  const { width, height } = frame;
  const image = { x: BORDER, y: BORDER, width, height };
  const along = (
    colors: readonly Rgb[],
    length: number,
    place: (start: number, size: number) => Rect,
  ) =>
    colors.map((color, index) => {
      // Whole pixels that meet exactly, so no seams show between colours.
      const start = Math.floor((index * length) / colors.length);
      const end = Math.floor(((index + 1) * length) / colors.length);
      return { ...place(BORDER + start, end - start), color };
    });
  const strips = [
    ...along(frame.colors.top, width, (x, size) => ({
      x,
      y: 0,
      width: size,
      height: STRIP,
    })),
    ...along(frame.colors.bottom, width, (x, size) => ({
      x,
      y: BORDER + height + GAP,
      width: size,
      height: STRIP,
    })),
    ...along(frame.colors.left, height, (y, size) => ({
      x: 0,
      y,
      width: STRIP,
      height: size,
    })),
    ...along(frame.colors.right, height, (y, size) => ({
      x: BORDER + width + GAP,
      y,
      width: STRIP,
      height: size,
    })),
  ];
  const { viewport, source_width, source_height } = frame;
  const whole =
    viewport.x === 0 &&
    viewport.y === 0 &&
    viewport.width === source_width &&
    viewport.height === source_height;
  const sx = source_width ? width / source_width : 0;
  const sy = source_height ? height / source_height : 0;
  return {
    width: width + 2 * BORDER,
    height: height + 2 * BORDER,
    image,
    strips,
    /** The sampled area on the canvas, or `null` when it is the whole frame. */
    viewport: whole
      ? null
      : {
          x: BORDER + viewport.x * sx,
          y: BORDER + viewport.y * sy,
          width: viewport.width * sx,
          height: viewport.height * sy,
        },
  };
}

/**
 * Draw one preview. `image` is the decoded JPEG, or `null` where the browser
 * cannot decode it (the colours are still drawn).
 */
export function drawPreview(
  canvas: HTMLCanvasElement,
  image: CanvasImageSource | null,
  frame: PreviewFrame,
) {
  const layout = previewLayout(frame);
  // Resizing clears the canvas, so only do it when the size changes.
  if (canvas.width !== layout.width) canvas.width = layout.width;
  if (canvas.height !== layout.height) canvas.height = layout.height;
  const context = canvas.getContext('2d');
  if (!context) return;
  context.clearRect(0, 0, layout.width, layout.height);
  for (const { x, y, width, height, color } of layout.strips) {
    context.fillStyle = `rgb(${color[0]}, ${color[1]}, ${color[2]})`;
    context.fillRect(x, y, width, height);
  }
  const { x, y, width, height } = layout.image;
  context.fillStyle = '#15161f';
  context.fillRect(x, y, width, height);
  if (image) context.drawImage(image, x, y, width, height);
  const viewport = layout.viewport;
  if (!viewport) return;
  // Dim what was cut off and outline what was sampled.
  context.fillStyle = 'rgba(0, 0, 0, 0.55)';
  context.beginPath();
  context.rect(x, y, width, height);
  context.rect(viewport.x, viewport.y, viewport.width, viewport.height);
  context.fill('evenodd');
  context.setLineDash([6, 4]);
  context.lineWidth = 2;
  context.strokeStyle = '#ffffff';
  context.strokeRect(
    viewport.x + 1,
    viewport.y + 1,
    viewport.width - 2,
    viewport.height - 2,
  );
  context.setLineDash([]);
}
