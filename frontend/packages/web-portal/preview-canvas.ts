import type { PreviewFrame } from '@prysm/application-client';

/**
 * Draw one preview's picture at its own size. `image` is the decoded JPEG, or
 * `null` where the browser cannot decode it (nothing is drawn).
 */
export function drawPicture(
  canvas: HTMLCanvasElement,
  image: CanvasImageSource | null,
  frame: PreviewFrame,
) {
  if (canvas.width !== frame.width) canvas.width = frame.width;
  if (canvas.height !== frame.height) canvas.height = frame.height;
  const context = canvas.getContext('2d');
  if (!context) return null;
  context.clearRect(0, 0, frame.width, frame.height);
  if (image) context.drawImage(image, 0, 0, frame.width, frame.height);
  return context;
}

/**
 * The part of the picture the colours were sampled from, in canvas pixels,
 * or `null` when it is the whole frame (no black bars removed).
 */
export function sampledArea(frame: PreviewFrame) {
  const { viewport, source_width, source_height, width, height } = frame;
  const whole =
    viewport.x === 0 &&
    viewport.y === 0 &&
    viewport.width === source_width &&
    viewport.height === source_height;
  if (whole) return null;
  const sx = source_width ? width / source_width : 0;
  const sy = source_height ? height / source_height : 0;
  return {
    x: viewport.x * sx,
    y: viewport.y * sy,
    width: viewport.width * sx,
    height: viewport.height * sy,
  };
}

/**
 * Draw the picture with the black bars Prysm removed dimmed and the sampled
 * area outlined, for checking black-bar removal.
 */
export function drawPreview(
  canvas: HTMLCanvasElement,
  image: CanvasImageSource | null,
  frame: PreviewFrame,
) {
  const context = drawPicture(canvas, image, frame);
  const area = sampledArea(frame);
  if (!context || !area) return;
  context.fillStyle = 'rgba(0, 0, 0, 0.55)';
  context.beginPath();
  context.rect(0, 0, frame.width, frame.height);
  context.rect(area.x, area.y, area.width, area.height);
  context.fill('evenodd');
  context.setLineDash([6, 4]);
  context.lineWidth = 2;
  context.strokeStyle = '#ffffff';
  context.strokeRect(area.x + 1, area.y + 1, area.width - 2, area.height - 2);
  context.setLineDash([]);
}
