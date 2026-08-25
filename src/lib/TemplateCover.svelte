<script lang="ts">
  /* The page a template shows on its card.

     No compile behind this and no measuring: the page is the PDF the document
     had already built when the template was saved, and its size was stored with
     it. So a card costs one raster of one page, and a gallery of them costs no
     round trips at all.

     Drawn at the size it is given rather than sized by its width. Templates
     change the paper — a beamer slide is landscape and a pocket book is neither
     shape — and scaling every page to the same width makes a shelf of cards no
     two of which are the same height. Callers fit the page into a fixed cell
     and pass what came out, so they know where its edges landed: a ring goes
     round the paper and not round the cell. */

  import { fetchPage, coverUrl } from '$lib/pdf';
  import type { PageVisibility, PageVisibilityTracker } from '$lib/pdf-visibility';
  import type { TemplateCover } from '$lib/types';

  let {
    id,
    page = 0,
    cover,
    width,
    height,
    invert = false,
    tracker = null
  }: {
    id: number;
    /** Which page of the PDF the template kept. A card shows the first. */
    page?: number;
    cover: TemplateCover;
    /** The box the page is fitted into, not the size it is drawn at. */
    width: number;
    height: number;
    invert?: boolean;
    /**
     * The shared observers, when this is one page of a column of them.
     *
     * Without one the page is drawn as soon as it is mounted, which is what a
     * card on a shelf wants: it is on screen, and there is one of it. With one
     * it waits until it is near the window and hands its bitmap back once it is
     * well past — a template of two hundred pages, drawn all at once, is two
     * hundred renders and a gigabyte of them.
     */
    tracker?: PageVisibilityTracker | null;
  } = $props();

  let host = $state<HTMLElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let failed = $state(false);
  /** What is painted, as a URL, so a redraw only happens when it must. */
  let painted = $state<string | null>(null);
  /** Whether this page is near enough to draw, and near enough to keep. */
  let visibility = $state<PageVisibility>({ render: false, retain: false });
  /* Both are true for good without a tracker: a card is mounted because it is
     being shown, and there is one of it. */
  const shown = $derived(!tracker || visibility.render);
  const kept = $derived(!tracker || visibility.retain);

  $effect(() => {
    const element = host;
    if (!element || !tracker) return;
    const observers = tracker;
    observers.observe(element, (next) => (visibility = next));
    return () => observers.unobserve(element);
  });

  // Far enough away that the bitmap is no longer worth its memory. The bands
  // overlap on purpose — see pdf-visibility — so a page gives its drawing back
  // long after it stops being drawn, and a small scroll back costs nothing.
  $effect(() => {
    if (kept || !canvas || painted === null) return;
    canvas.width = 0;
    canvas.height = 0;
    painted = null;
  });

  $effect(() => {
    const surface = canvas;
    if (!shown) return;
    const url = coverUrl(
      id,
      page,
      Math.min(
        Math.min(width / cover.width, height / cover.height) * (window.devicePixelRatio || 1),
        4
      ),
      invert
    );
    if (!surface || painted === url) return;

    const controller = new AbortController();
    void (async () => {
      try {
        const page = await fetchPage(url, controller.signal);
        if (controller.signal.aborted) {
          page.bitmap.close();
          return;
        }
        const ratio = window.devicePixelRatio || 1;
        surface.width = page.width;
        surface.height = page.height;
        surface.style.width = `${page.width / ratio}px`;
        surface.style.height = `${page.height / ratio}px`;
        surface.getContext('2d', { alpha: false })?.drawImage(page.bitmap, 0, 0);
        page.bitmap.close();
        painted = url;
        failed = false;
      } catch {
        // Not worth a message. A card that cannot draw its page still carries
        // the name, and the template is no less usable for it.
        if (!controller.signal.aborted) failed = true;
      }
    })();

    return () => controller.abort();
  });
</script>

<span
  bind:this={host}
  class="cover"
  class:inverted={invert}
  style="width: {width}px; height: {height}px"
>
  <canvas bind:this={canvas} class:hidden={!painted || failed}></canvas>
</span>

<style>
  /* The box, and it is the page: callers size it to the paper — see `fitPage` —
     so what it carries is what every page-shaped card in Press carries. The
     radius, the sheet and the shadow are .pinned-thumb's exactly, because this
     is that card at that size, and a template's cover has no business being
     drawn differently from a document's. The sheet is under the drawing rather
     than behind the whole cell, so it is paper the moment the card appears and
     an inverted page never flashes white.

     The ring is the caller's, handed down as --cover-ring. It belongs here and
     not on the cell, so that it follows the paper's corners — a landscape
     template ringed at the cell's edges is ringed around empty room.

     So is the radius, through --cover-radius, for the one caller that is not a
     card: a page being read has square corners here as it does in the reader,
     and rounding a full-height page would be rounding the paper itself. */
  .cover {
    display: grid;
    place-items: center;
    overflow: hidden;
    border-radius: var(--cover-radius, var(--radius));
    background: var(--sheet);
    box-shadow: var(--cover-ring, 0 0 #0000), var(--shadow-md);
    transition: box-shadow var(--duration);
  }

  .cover.inverted {
    background: var(--sheet-inverted);
  }

  /* The drawing, rounded off with the box it fills. The hairline stays, and is
     the one thing here the shelf does not have: the shelf lays its pages on a
     band darker than they are, and this lays them on a dialog that is the same
     white the paper is. The shadow alone would leave the page without an edge.

     The minimums close the fraction of a pixel that rounding can leave between
     the drawing and the sheet under it — the same floor the library's
     thumbnails set, for the same reason. */
  canvas {
    display: block;
    grid-area: 1 / 1;
    min-width: 100%;
    min-height: 100%;
    border: var(--bw) solid var(--line-2);
    border-radius: var(--cover-radius, var(--radius));
  }

  canvas.hidden {
    display: none;
  }
</style>
