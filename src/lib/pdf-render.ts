import type { PDFPageProxy, RenderTask } from 'pdfjs-dist';

export const MAX_CANVAS_PIXELS = 12_000_000;

export type StartedPageRender = {
  width: number;
  height: number;
  task: RenderTask;
};

export function startPageRender(
  page: PDFPageProxy,
  canvas: HTMLCanvasElement,
  scale: number,
  deviceScale: number
): StartedPageRender {
  const viewport = page.getViewport({ scale });
  const requestedScale = Number.isFinite(deviceScale) && deviceScale > 0 ? deviceScale : 1;
  const viewportPixels = Math.max(1, viewport.width * viewport.height);
  const outputScale = Math.min(requestedScale, Math.sqrt(MAX_CANVAS_PIXELS / viewportPixels));

  canvas.width = Math.max(1, Math.floor(viewport.width * outputScale));
  canvas.height = Math.max(1, Math.floor(viewport.height * outputScale));
  canvas.style.width = `${Math.floor(viewport.width)}px`;
  canvas.style.height = `${Math.floor(viewport.height)}px`;

  const transform =
    outputScale === 1 ? undefined : [outputScale, 0, 0, outputScale, 0, 0];
  const task = page.render({ canvas, viewport, transform });
  return { width: viewport.width, height: viewport.height, task };
}
