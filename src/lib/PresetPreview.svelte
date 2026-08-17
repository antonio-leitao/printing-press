<script lang="ts">
  /* What a preset does, drawn.

     A frontmatter block is a preamble in disguise and nobody can read one and
     say what the page will look like, so the choice is made by looking.

     It checks the preset at the same time: whatever will not compile fails
     here, in front of the person who just typed it, rather than at the next
     build of a real document. `onerror` hands the reason up so the card can
     stay small and the full complaint can be shown wherever there is room.

     Debounced, because the body arrives a keystroke at a time and each attempt
     is a real TeX run. Previews are addressed by the hash of the body, so
     returning to something already compiled costs a lookup rather than a
     rebuild. */

  import { FileWarning } from '@lucide/svelte';

  import { api, errorMessage } from '$lib/api';
  import { fetchPage, previewUrl } from '$lib/pdf';
  import { theme } from '$lib/theme.svelte';

  let {
    body,
    width = 132,
    height = 176,
    onerror
  }: {
    body: string;
    /** The box a page is fitted into, not the size the page is drawn at. */
    width?: number;
    height?: number;
    /** Called with the complaint, or with '' once it compiles. */
    onerror?: (reason: string) => void;
  } = $props();

  /** Long enough that typing does not queue a TeX run per character, short
      enough that choosing a template from a list still feels like a click —
      the same wait now serves both, and a compiled preset is only a lookup. */
  const SETTLE = 450;

  let canvas = $state<HTMLCanvasElement | null>(null);
  let failed = $state(false);
  let working = $state(false);
  /** What is painted, as a URL, so a redraw only happens when it must — and so
      the panel knows whether it has a page yet. */
  let painted = $state<string | null>(null);

  const dark = $derived(theme.isDark);

  $effect(() => {
    const surface = canvas;
    const text = body;
    const invert = dark;
    const box = { width, height };
    if (!surface) return;

    const controller = new AbortController();
    const timer = setTimeout(() => {
      void (async () => {
        working = true;
        try {
          const preview = await api.previewPreset(text);
          if (controller.signal.aborted) return;
          const ratio = window.devicePixelRatio || 1;
          // Fitted to the box rather than sized by its width. Presets change
          // the paper — one of them is 5.5x8in — and scaling every page to the
          // same width makes a shelf of cards no two of which are the same
          // height. Fitting means the box is the constant and the page inside
          // it is what varies, which is also what the choice is about.
          const fit = Math.min(box.width / preview.width, box.height / preview.height);
          const scale = Math.min(fit * ratio, 4);
          const url = previewUrl(preview.digest, 0, scale, invert);
          failed = false;
          onerror?.('');
          // The last drawing stays up while a new one compiles, so editing
          // never blinks the card empty between keystrokes.
          if (painted === url) return;
          const page = await fetchPage(url, controller.signal);
          if (controller.signal.aborted) {
            page.bitmap.close();
            return;
          }
          surface.width = page.width;
          surface.height = page.height;
          surface.style.width = `${page.width / ratio}px`;
          surface.style.height = `${page.height / ratio}px`;
          surface.getContext('2d', { alpha: false })?.drawImage(page.bitmap, 0, 0);
          page.bitmap.close();
          painted = url;
        } catch (reason) {
          if (controller.signal.aborted) return;
          // Not a failure of Press. This is pandoc or TeX saying what is wrong
          // with the preset, which is the whole reason to compile one here.
          failed = true;
          onerror?.(errorMessage(reason));
        } finally {
          if (!controller.signal.aborted) working = false;
        }
      })();
    }, SETTLE);

    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  });
</script>

<span
  class="page"
  class:working
  class:failed
  style="width: {width}px; height: {height}px"
>
  <canvas bind:this={canvas} class:hidden={!painted}></canvas>
  {#if failed}
    <span class="mark"><FileWarning size={18} strokeWidth={1.75} aria-hidden="true" /></span>
  {:else if !painted}
    <span class="mark quiet">…</span>
  {/if}
</span>

<style>
  /* A fixed box, with the page centred inside it, so switching to a preset on
     different paper does not move everything below it. The box itself is not
     drawn: a tinted panel showing either side of a narrow page reads as a mount
     around the page rather than as empty room, and the paper is the one thing
     here that should look like paper. */
  .page {
    display: grid;
    place-items: center;
    transition: opacity var(--duration);
  }

  /* Dimmed rather than emptied while a new one compiles: the page on screen is
     still the truth about the preset as it was a moment ago. */
  .page.working {
    opacity: 0.45;
  }

  /* A page that no longer matches what is typed should not look current. */
  .page.failed canvas {
    opacity: 0.25;
  }

  /* The border and the shadow belong to the page, not to the box that holds
     it, or they would outline empty space beside a narrow one. */
  canvas {
    display: block;
    grid-area: 1 / 1;
    border: var(--bw) solid var(--line-2);
    box-shadow: var(--shadow-sm);
  }

  canvas.hidden {
    display: none;
  }

  .mark {
    grid-area: 1 / 1;
    display: grid;
    place-items: center;
    color: var(--danger);
  }

  .mark.quiet {
    color: var(--ink-3);
  }
</style>
