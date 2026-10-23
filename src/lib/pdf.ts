/**
 * Fetching rasterised pages from the native renderer.
 *
 * Pages arrive as raw RGBA over the `press:` scheme rather than as PNG. There is
 * no image decode to do: the bytes go straight into an `ImageData` and then into
 * an `ImageBitmap`, which is handed to the canvas whole rather than drawn into
 * it. Encoding a page to PNG in Rust would cost more than MuPDF spent drawing it.
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
 * A page of the PDF a template kept.
 *
 * Page zero is the cover every card shows. The rest exist for a closer look,
 * which reads a template the way the reader reads a document. No revision here,
 * unlike `pageUrl`: what a template kept was copied once and cannot be rebuilt,
 * so there is nothing for a stale cache to be stale against.
 */
export function coverUrl(
  templateId: number,
  page: number,
  scale: number,
  invert = false
): string {
  const ink = invert ? '&invert=1' : '';
  return `${ORIGIN}/cover/${templateId}/${page}?scale=${scale.toFixed(4)}${ink}`;
}

/**
 * Bytes of dimensions in front of the samples: width then height, little-endian
 * `u32`. They travel in the body rather than in headers because this response is
 * cross-origin, and a cross-origin reader cannot see custom headers unless the
 * server also exposes them.
 */
const PAGE_HEADER_BYTES = 8;

/**
 * A page that did not arrive, and whether asking again could help.
 *
 * The renderer refuses a malformed request (400) and a document that has gone
 * (404), and those will be refused again. Anything else is usually brief: a
 * render the pool gave up on because a scroll ran ahead of it, or a PDF read in
 * the middle of being rewritten by whatever owns it.
 */
class PageError extends Error {
  constructor(
    message: string,
    readonly retryable: boolean
  ) {
    super(message);
  }
}

/**
 * How long to wait before each further attempt at a page that failed. Short
 * first, because the usual cause has passed by the time anyone could notice.
 */
const RETRY_DELAYS = [250, 1000, 3000];

/** Resolves after `ms`, or rejects as soon as `signal` is aborted. */
function pause(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', stop);
      resolve();
    }, ms);
    const stop = () => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    signal?.addEventListener('abort', stop, { once: true });
  });
}

/**
 * One page, asked for again a few times if it fails in a way that might not
 * last. Without this a page that missed once showed its error until the reader
 * scrolled it away and back, zoomed, or waited for a rebuild.
 */
export async function fetchPage(url: string, signal?: AbortSignal): Promise<PageBitmap> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await fetchPageOnce(url, signal);
    } catch (reason) {
      const retryable = reason instanceof PageError ? reason.retryable : reason instanceof TypeError;
      if (signal?.aborted || !retryable || attempt >= RETRY_DELAYS.length) throw reason;
      await pause(RETRY_DELAYS[attempt], signal);
    }
  }
}

async function fetchPageOnce(url: string, signal?: AbortSignal): Promise<PageBitmap> {
  const response = await fetch(url, { signal });
  if (!response.ok) {
    throw new PageError(
      (await response.text()) || `page request failed (${response.status})`,
      response.status >= 500
    );
  }
  const buffer = await response.arrayBuffer();
  if (buffer.byteLength < PAGE_HEADER_BYTES) {
    throw new PageError('the renderer returned a truncated page', true);
  }
  const header = new DataView(buffer, 0, PAGE_HEADER_BYTES);
  const width = header.getUint32(0, true);
  const height = header.getUint32(4, true);
  const expected = width * height * 4;
  if (width <= 0 || height <= 0 || buffer.byteLength - PAGE_HEADER_BYTES !== expected) {
    throw new PageError(
      `expected ${expected} bytes for a ${width}x${height} page, received ${
        buffer.byteLength - PAGE_HEADER_BYTES
      }`,
      true
    );
  }
  // A view over the samples, not a copy of them.
  const samples = new Uint8ClampedArray(buffer, PAGE_HEADER_BYTES, expected);
  const image = new ImageData(samples, width, height);
  return { bitmap: await createImageBitmap(image), width, height };
}

/**
 * Hands a drawn page to a canvas and sizes the canvas to it.
 *
 * The bitmap is given away rather than drawn: a `bitmaprenderer` context takes
 * ownership of it, where `drawImage` into a 2d context made a second copy of a
 * page that is twelve megabytes at ordinary zoom on a retina display. The
 * bitmap is spent afterwards either way, so the caller has nothing to close.
 */
export function paint(canvas: HTMLCanvasElement, page: PageBitmap) {
  canvas.width = page.width;
  canvas.height = page.height;
  // Null on a canvas that already has a 2d context, which is the one this
  // falls back to, so a canvas keeps whichever kind it started with.
  const renderer = canvas.getContext('bitmaprenderer');
  if (renderer) {
    renderer.transferFromImageBitmap(page.bitmap);
    return;
  }
  canvas.getContext('2d', { alpha: false })?.drawImage(page.bitmap, 0, 0);
  page.bitmap.close();
}

/** Gives a canvas's drawing back, for a page far enough away to lose it. */
export function blank(canvas: HTMLCanvasElement) {
  canvas.getContext('bitmaprenderer')?.transferFromImageBitmap(null);
  canvas.width = 0;
  canvas.height = 0;
}
