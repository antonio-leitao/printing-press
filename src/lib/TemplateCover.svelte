<script lang="ts">
  /* The page a template shows on its card.

     No compile behind this and no measuring: the page is the PDF the document
     had already built when the template was saved, and its size was stored with
     it. So a card costs one raster of one page, and a gallery of them costs no
     round trips at all.

     Fitted into a fixed box rather than sized by its width. Templates change
     the paper — a beamer slide is landscape and a pocket book is neither shape
     — and scaling every page to the same width makes a shelf of cards no two of
     which are the same height. The box is the constant; the page inside it is
     what varies, which is also what the choice is about. */

  import { fetchPage, coverUrl } from '$lib/pdf';
  import type { TemplateCover } from '$lib/types';

  let {
    id,
    cover,
    width,
    height,
    invert = false
  }: {
    id: number;
    cover: TemplateCover;
    /** The box the page is fitted into, not the size it is drawn at. */
    width: number;
    height: number;
    invert?: boolean;
  } = $props();

  let canvas = $state<HTMLCanvasElement | null>(null);
  let failed = $state(false);
  /** What is painted, as a URL, so a redraw only happens when it must. */
  let painted = $state<string | null>(null);

  $effect(() => {
    const surface = canvas;
    const url = coverUrl(
      id,
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

<span class="cover" style="width: {width}px; height: {height}px">
  <canvas bind:this={canvas} class:hidden={!painted || failed}></canvas>
</span>

<style>
  /* A fixed box with the page centred in it, so a landscape card and an upright
     one sit on the same line. The box itself is not drawn: what should look
     like paper is the paper. */
  .cover {
    display: grid;
    place-items: center;
  }

  /* The border and the shadow belong to the page, not to the box, or they would
     outline empty room beside a narrow one. */
  canvas {
    display: block;
    grid-area: 1 / 1;
    border: var(--bw) solid var(--line-2);
    box-shadow: var(--shadow-sm);
  }

  canvas.hidden {
    display: none;
  }
</style>
