export const MIN_ZOOM = 0.25;
export const MAX_ZOOM = 5;
export const ZOOM_STEP = 1.1;

export function normalizeZoom(value: number): number {
  if (!Number.isFinite(value)) return 1;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Math.round(value * 100) / 100));
}

export function zoomBySteps(current: number, steps: number): number {
  return normalizeZoom(current * ZOOM_STEP ** steps);
}

export function wheelZoomFactor(deltaPixels: number): number {
  const limitedDelta = Math.min(10, Math.max(-10, deltaPixels));
  return Math.exp(-limitedDelta * 0.01);
}
