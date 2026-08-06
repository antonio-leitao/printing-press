<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage } from '$lib/api';
  import { fetchPage, pageUrl } from '$lib/pdf';
  import type { ArtifactSummary } from '$lib/types';

  let { artifact } = $props<{ artifact: ArtifactSummary }>();

  let host = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let renderError = $state('');

  onMount(() => {
    const element = host;
    if (!element) return;
    const controller = new AbortController();
    let started = false;

    const render = async () => {
      if (started || !canvas) return;
      started = true;
      const surface = canvas;
      try {
        const [first] = await api.pageLayout(artifact.id);
        if (!first || controller.signal.aborted) return;
        // Small enough to be cheap, sharp enough on a retina display.
        const scale = Math.min(260 / first.width, 0.5) * (window.devicePixelRatio || 1);
        const page = await fetchPage(
          pageUrl(artifact.id, artifact.revision, 0, scale),
          controller.signal
        );
        if (controller.signal.aborted) {
          page.bitmap.close();
          return;
        }
        surface.width = page.width;
        surface.height = page.height;
        surface.style.width = `${Math.round(page.width / (window.devicePixelRatio || 1))}px`;
        surface.getContext('2d', { alpha: false })?.drawImage(page.bitmap, 0, 0);
        page.bitmap.close();
      } catch (reason) {
        if (!controller.signal.aborted) renderError = errorMessage(reason);
      }
    };

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          observer.disconnect();
          void render();
        }
      },
      { rootMargin: '400px' }
    );
    observer.observe(element);
    return () => {
      controller.abort();
      observer.disconnect();
    };
  });
</script>

<div class="thumbnail" bind:this={host} aria-label="Last compiled PDF thumbnail">
  {#if renderError}<span title={renderError}>Preview unavailable</span>{/if}
  <canvas bind:this={canvas}></canvas>
</div>

<style>
  .thumbnail {
    display: grid;
    min-height: 180px;
    place-items: center;
    overflow: hidden;
    background: #e7e7e7;
  }

  canvas {
    display: block;
    max-width: 100%;
    box-shadow: 0 1px 4px #0004;
  }
</style>
