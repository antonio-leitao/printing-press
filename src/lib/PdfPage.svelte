<script lang="ts">
  import { errorMessage } from '$lib/api';
  import { fetchPage, pageUrl, renderScale } from '$lib/pdf';
  import type { PageVisibility, PageVisibilityTracker } from '$lib/pdf-visibility';
  import type { ArtifactSummary, PageSize } from '$lib/types';

  let { artifact, size, pageNumber, zoom, tracker } = $props<{
    artifact: ArtifactSummary;
    size: PageSize;
    pageNumber: number;
    zoom: number;
    tracker: PageVisibilityTracker;
  }>();

  let host = $state<HTMLElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let visibility = $state<PageVisibility>({ render: false, retain: false });
  let renderError = $state('');
  /** Identifies what is currently painted, so a redraw only happens if needed. */
  let painted: string | null = null;

  const width = $derived(size.width * zoom);
  const height = $derived(size.height * zoom);

  $effect(() => {
    const element = host;
    if (!element) return;
    tracker.observe(element, (next: PageVisibility) => (visibility = next));
    return () => tracker.unobserve(element);
  });

  // Far enough away that the bitmap is no longer worth its memory. A page at
  // retina scale is several megabytes; without this a long document keeps every
  // page it has ever shown.
  $effect(() => {
    if (visibility.retain || !canvas || painted === null) return;
    canvas.width = 0;
    canvas.height = 0;
    painted = null;
  });

  $effect(() => {
    if (!visibility.render || !canvas) return;
    const scale = renderScale(size.width, size.height, zoom, window.devicePixelRatio || 1);
    const url = pageUrl(artifact.id, artifact.revision, pageNumber - 1, scale);
    // The previous drawing stays on screen until the new one replaces it, so a
    // rebuild or a zoom never flashes an empty page.
    if (painted === url) return;

    const surface = canvas;
    const controller = new AbortController();
    renderError = '';

    void (async () => {
      try {
        const page = await fetchPage(url, controller.signal);
        if (controller.signal.aborted) {
          page.bitmap.close();
          return;
        }
        surface.width = page.width;
        surface.height = page.height;
        const context = surface.getContext('2d', { alpha: false });
        if (!context) throw new Error('could not get a drawing context');
        context.drawImage(page.bitmap, 0, 0);
        // The bitmap has been copied into the canvas; holding it would double
        // the memory for every visible page.
        page.bitmap.close();
        painted = url;
      } catch (reason) {
        if (controller.signal.aborted) return;
        renderError = errorMessage(reason);
      }
    })();

    return () => controller.abort();
  });
</script>

<section
  class="page"
  bind:this={host}
  style:width={`${width}px`}
  style:aspect-ratio={`${width} / ${height}`}
  data-page={pageNumber}
  aria-label={`Page ${pageNumber}`}
>
  {#if renderError}
    <p class="render-error" role="alert">Page {pageNumber} could not render: {renderError}</p>
  {/if}
  <canvas bind:this={canvas}></canvas>
</section>

<style>
  /* Bottom margin only, so the first page sits flush against the top of the
     window and the gap exists only *between* pages. PAGE_GUTTER in
     PdfViewer.svelte is this number. */
  .page {
    margin: 0 auto 16px;
    background: white;
    box-shadow: 0 0.4px 2px rgba(0, 0, 0, 0.25);
  }

  canvas {
    display: block;
    width: 100% !important;
    height: auto !important;
  }

  .render-error {
    margin: 0;
    padding: 1rem;
    color: #900;
  }
</style>
