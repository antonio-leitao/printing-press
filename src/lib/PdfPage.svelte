<script lang="ts">
  import { api, errorMessage } from '$lib/api';
  import { fetchPage, pageUrl, renderScale } from '$lib/pdf';
  import type { PageVisibility, PageVisibilityTracker } from '$lib/pdf-visibility';
  import type { ArtifactSummary, LinkBox, PageSize } from '$lib/types';

  let { artifact, size, pageNumber, zoom, tracker, onFollow, invert = false } = $props<{
    artifact: ArtifactSummary;
    size: PageSize;
    pageNumber: number;
    zoom: number;
    tracker: PageVisibilityTracker;
    /** Called when a link on this page is clicked. */
    onFollow?: (link: LinkBox) => void;
    /** Drawn for a dark room: the page is inverted as it is rasterised. */
    invert?: boolean;
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

  // Fetched once per page, when it first comes near the window. Where the links
  // are does not change with the zoom — they are in the page's own points — so
  // the overlay is scaled rather than fetched again.
  let links = $state<LinkBox[]>([]);
  /// Which build's links are held. A rebuild moves things on the page, so the
  /// old ones would point a line or two off.
  let asked: string | null = null;

  $effect(() => {
    const build = `${artifact.id}:${artifact.revision}`;
    if (!visibility.render || asked === build) return;
    asked = build;
    void api
      .pageLinks(artifact.id, pageNumber - 1)
      .then((found) => (links = found))
      // A page whose links cannot be read is a page without links, not an error
      // worth putting in front of someone reading.
      .catch(() => (links = []));
  });

  $effect(() => {
    if (!visibility.render || !canvas) return;
    const scale = renderScale(size.width, size.height, zoom, window.devicePixelRatio || 1);
    const url = pageUrl(artifact.id, artifact.revision, pageNumber - 1, scale, invert);
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
  class:inverted={invert}
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
  <!-- Over the page rather than drawn into it, so a reference stays clickable
       at any zoom and the rasteriser stays a rasteriser. Modified clicks are
       left alone: cmd-click is the source peek. -->
  {#each links as link, index (index)}
    <button
      class="link-box"
      style:left={`${link.x * zoom}px`}
      style:top={`${link.y * zoom}px`}
      style:width={`${link.width * zoom}px`}
      style:height={`${link.height * zoom}px`}
      title={link.uri ?? (link.page ? `Page ${link.page}` : undefined)}
      aria-label={link.uri ?? `Go to page ${link.page}`}
      onclick={(event) => {
        if (event.metaKey || event.ctrlKey || event.altKey) return;
        event.preventDefault();
        onFollow?.(link);
      }}
    ></button>
  {/each}
</section>

<style>
  /* Bottom margin only, so the first page sits flush against the top of the
     window and the gap exists only *between* pages. PAGE_GUTTER in
     PdfViewer.svelte is this number. */
  .page {
    position: relative;
    margin: 0 auto 16px;
    background: var(--card);
    box-shadow: var(--shadow-md);
  }

  /* The paper under the drawing. It shows before a page has been drawn, and a
     white flash there is the one thing an inverted page must not do. */
  .page.inverted {
    background: #101010;
  }

  /* Invisible until the pointer is on it, and then only a wash: the document
     already shows its links however the author coloured them, and a second set
     of marks over the top would be someone else's typography. */
  .link-box {
    position: absolute;
    padding: 0;
    border: 0;
    border-radius: 2px;
    background: none;
    cursor: pointer;
  }

  .link-box:hover {
    background: var(--accent-wash);
  }

  canvas {
    display: block;
    width: 100% !important;
    height: auto !important;
  }

  .render-error {
    margin: 0;
    padding: 1rem;
    color: var(--danger-strong);
  }
</style>
