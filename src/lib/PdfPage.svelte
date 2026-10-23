<script lang="ts">
  import { api, errorMessage } from '$lib/api';
  import { display } from '$lib/display.svelte';
  import { blank, fetchPage, paint, pageUrl, renderScale } from '$lib/pdf';
  import type { PageVisibility, PageVisibilityTracker } from '$lib/pdf-visibility';
  import type { ArtifactSummary, LinkBox, PageSize, SearchHit } from '$lib/types';

  let {
    artifact,
    size,
    pageNumber,
    zoom,
    tracker,
    onFollow,
    invert = false,
    hits = [],
    current = null
  } = $props<{
    artifact: ArtifactSummary;
    size: PageSize;
    pageNumber: number;
    zoom: number;
    tracker: PageVisibilityTracker;
    /** Called when a link on this page is clicked. */
    onFollow?: (link: LinkBox) => void;
    /** Drawn for a dark room: the page is inverted as it is rasterised. */
    invert?: boolean;
    /** Search hits on this page, in PDF points from its top left. */
    hits?: SearchHit[];
    /** The hit the search is on, which may be on another page entirely. */
    current?: SearchHit | null;
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
    blank(canvas);
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
    // The display's ratio as state, so a window moved to another screen is
    // drawn again for it.
    const scale = renderScale(size.width, size.height, zoom, display.pixelRatio);
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
        paint(surface, page);
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
  <!-- Over the page like the links, and for the same reason: a highlight is the
       reader's mark, not the document's, and it has to stay put at any zoom.
       A point of slack on each side, because MuPDF's boxes are cut tight to
       the glyphs and a highlight that touches the letters reads as a smudge. -->
  {#each hits as hit, index (index)}
    <span
      class="hit"
      class:current={hit === current}
      style:left={`${(hit.x - 1) * zoom}px`}
      style:top={`${(hit.y - 1) * zoom}px`}
      style:width={`${(hit.width + 2) * zoom}px`}
      style:height={`${(hit.height + 2) * zoom}px`}
    ></span>
  {/each}
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
    background: var(--sheet);
    box-shadow: var(--shadow-md);
  }

  /* The paper under the drawing. It shows before a page has been drawn, and a
     white flash there is the one thing an inverted page must not do.

     Nothing else changes: the page is lighter than the ground it rests on here
     exactly as it is in the light, so it says where it ends by itself. The
     shadow stays for the same reason it is there in the light, and does much
     less — a contact shadow on a ground this dark has little to fall on. */
  .page.inverted {
    background: var(--sheet-inverted);
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

  /* Translucent rather than blended, so the same mark reads on paper and on a
     page drawn for the dark. Clicks go through to the link underneath. */
  .hit {
    position: absolute;
    border-radius: var(--radius-xs);
    background: color-mix(in oklab, var(--amber) 34%, transparent);
    pointer-events: none;
  }

  .hit.current {
    background: color-mix(in oklab, var(--accent) 42%, transparent);
    box-shadow: 0 0 0 var(--bw) var(--accent);
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
