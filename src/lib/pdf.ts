/**
 * Fetching rasterised pages from the native renderer.
 *
 * Pages arrive as raw RGBA over the `press:` scheme rather than as PNG. There is
 * no image decode to do: the bytes go straight into an `ImageData` and then into
 * an `ImageBitmap`, which the compositor can blit. Encoding a page to PNG in
 * Rust would cost more than MuPDF spent drawing it.
 */

/**
 * A ceiling on the pixels in one drawn page, so a large page at high zoom
 * cannot ask for a bitmap the GPU will refuse or the machine will choke on.
 */
export const MAX_PAGE_PIXELS = 16_000_000;

/**
 * Tauri serves custom schemes as `press://localhost` everywhere except Windows,
 * which rewrites them onto http.
 */
const ORIGIN = navigator.userAgent.includes('Windows')
  ? 'http://press.localhost'
  : 'press://localhost';

export type PageBitmap = {
  bitmap: ImageBitmap;
  width: number;
  height: number;
};

/**
 * Device pixels per PDF point for a page, clamped so one page can never exceed
 * the pixel budget. Returning the clamped value means the renderer is never
 * asked to draw pixels that would be thrown away.
 */
export function renderScale(
  pageWidth: number,
  pageHeight: number,
  zoom: number,
  devicePixelRatio: number
): number {
  const ratio = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0 ? devicePixelRatio : 1;
  const wanted = zoom * ratio;
  const pixels = pageWidth * pageHeight * wanted * wanted;
  if (!Number.isFinite(pixels) || pixels <= MAX_PAGE_PIXELS) return wanted;
  return wanted * Math.sqrt(MAX_PAGE_PIXELS / pixels);
}

/** The revision is in the path so a rebuilt page can never be served stale. */
export function pageUrl(
  artifactId: number,
  revision: number,
  page: number,
  scale: number,
  invert = false
): string {
  // Part of the address, so a page drawn one way is never mistaken for the
  // same page drawn the other: the viewer keeps what it has painted by URL.
  const ink = invert ? '&invert=1' : '';
  return `${ORIGIN}/page/${artifactId}/${revision}/${page}?scale=${scale.toFixed(4)}${ink}`;
}

/**
 * Bytes of dimensions in front of the samples: width then height, little-endian
 * `u32`. They travel in the body rather than in headers because this response is
 * cross-origin, and a cross-origin reader cannot see custom headers unless the
 * server also exposes them.
 */
const PAGE_HEADER_BYTES = 8;

export async function fetchPage(url: string, signal?: AbortSignal): Promise<PageBitmap> {
  const response = await fetch(url, { signal });
  if (!response.ok) {
    throw new Error((await response.text()) || `page request failed (${response.status})`);
  }
  const buffer = await response.arrayBuffer();
  if (buffer.byteLength < PAGE_HEADER_BYTES) {
    throw new Error('the renderer returned a truncated page');
  }
  const header = new DataView(buffer, 0, PAGE_HEADER_BYTES);
  const width = header.getUint32(0, true);
  const height = header.getUint32(4, true);
  const expected = width * height * 4;
  if (width <= 0 || height <= 0 || buffer.byteLength - PAGE_HEADER_BYTES !== expected) {
    throw new Error(
      `expected ${expected} bytes for a ${width}x${height} page, received ${
        buffer.byteLength - PAGE_HEADER_BYTES
      }`
    );
  }
  // A view over the samples, not a copy of them.
  const samples = new Uint8ClampedArray(buffer, PAGE_HEADER_BYTES, expected);
  const image = new ImageData(samples, width, height);
  return { bitmap: await createImageBitmap(image), width, height };
}
