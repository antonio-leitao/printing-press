<script lang="ts">
  import { api, errorMessage } from '$lib/api';
  import { fetchPage, pageUrl } from '$lib/pdf';
  import type { ArtifactSummary, PageSize } from '$lib/types';

  /**
   * `width` is roughly the width the thumbnail is shown at, so nothing larger
   * than that is ever drawn. It has to be passed by whoever sizes the container,
   * or a small thumbnail costs a full-page render and a large one is blurred.
   * The height is read from the container, which has one.
   */
  let { artifact, width = 56, invert = false } = $props<{
    artifact: ArtifactSummary;
    width?: number;
    /** Drawn for a dark room, the same way the viewer draws a page. */
    invert?: boolean;
  }>();

  let host = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let renderError = $state('');
  /** Whether this has ever come near the window. */
  let near = $state(false);
  /** What is currently painted, as a URL, so a redraw only happens if needed. */
  let painted: string | null = null;
  /// The first page's size, kept per build. It cannot change without the
  /// revision changing, so switching theme redraws without asking again.
  let measured: { build: string; size: PageSize } | null = null;

  // Watched until it has been seen once. After that the page is paid for and
  // the observer has nothing left to say — the library is scrolled up and down.
  $effect(() => {
    const element = host;
    if (!element || near) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) near = true;
      },
      { rootMargin: '400px' }
    );
    observer.observe(element);
    return () => observer.disconnect();
  });

  // Drawn when it comes near, and again when what it should show changes: a
  // rebuild publishes a new revision, and the theme decides which way round the
  // page is drawn. Keyed by URL like the viewer's own pages, so everything else
  // — a scroll, a re-render, a sibling arriving — costs nothing.
  $effect(() => {
    const surface = canvas;
    const element = host;
    if (!near || !surface || !element) return;
    const build = `${artifact.id}:${artifact.revision}`;
    const dark = invert;

    const controller = new AbortController();
    void (async () => {
      try {
        const first =
          measured?.build === build ? measured.size : (await api.pageLayout(artifact.id))[0];
        if (!first || controller.signal.aborted) return;
        measured = { build, size: first };
        // Small enough to be cheap, sharp enough on a retina display. Sized to
        // cover the box rather than fit inside it, so the scale follows
        // whichever side needs more of the page: the width for an upright page,
        // the height for a landscape one, which would otherwise be drawn at a
        // third of the pixels it is shown at.
        const box = element.getBoundingClientRect();
        const cover = Math.max(width / first.width, box.height / first.height || 0);
        const scale = Math.min(cover, 1) * (window.devicePixelRatio || 1);
        const url = pageUrl(artifact.id, artifact.revision, 0, scale, dark);
        // The previous drawing stays up until the new one replaces it, so
        // changing theme never blinks a library full of empty slots.
        if (painted === url) return;

        const page = await fetchPage(url, controller.signal);
        if (controller.signal.aborted) {
          page.bitmap.close();
          return;
        }
        surface.width = page.width;
        surface.height = page.height;
        // Shown at the size it was drawn for. Because that size covers the box,
        // whatever hangs over the edge is what the box crops away.
        const ratio = window.devicePixelRatio || 1;
        surface.style.width = `${page.width / ratio}px`;
        surface.style.height = `${page.height / ratio}px`;
        surface.getContext('2d', { alpha: false })?.drawImage(page.bitmap, 0, 0);
        page.bitmap.close();
        painted = url;
        renderError = '';
      } catch (reason) {
        if (!controller.signal.aborted) renderError = errorMessage(reason);
      }
    })();

    return () => controller.abort();
  });
</script>

<div class="thumbnail" bind:this={host} aria-label="Last compiled PDF thumbnail">
  {#if renderError}<span title={renderError}>Preview unavailable</span>{/if}
  <canvas bind:this={canvas}></canvas>
</div>

<style>
  /* Sized by whatever contains it, so the caller decides how big a thumbnail is.
     Aligned to the top, because a page that overflows this box should lose its
     foot rather than be trimmed at both ends. */
  .thumbnail {
    display: grid;
    width: 100%;
    height: 100%;
    place-items: start center;
    overflow: hidden;
    font-size: var(--fs-meta);
  }

  /* The size is set on the element as it is drawn, above. These are only a
     floor, closing the fraction of a pixel that rounding can leave between the
     page and the edge of its box. */
  canvas {
    display: block;
    min-width: 100%;
    min-height: 100%;
  }
</style>
