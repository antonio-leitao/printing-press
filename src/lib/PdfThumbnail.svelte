<script lang="ts">
  import { onMount } from 'svelte';
  import type { RenderTask } from 'pdfjs-dist';
  import { errorMessage } from '$lib/api';
  import { loadProjectPdf } from '$lib/pdf';
  import { startPageRender } from '$lib/pdf-render';

  let { projectId, revision } = $props<{
    projectId: number;
    revision: number;
  }>();

  let host: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let renderError = $state('');

  onMount(() => {
    let cancelled = false;
    let started = false;
    let renderTask: RenderTask | undefined;
    const render = async () => {
      if (started) return;
      started = true;
      try {
        const document = await loadProjectPdf(projectId, revision);
        const page = await document.getPage(1);
        if (cancelled) return;
        const base = page.getViewport({ scale: 1 });
        const scale = Math.min(260 / base.width, 0.5);
        const startedRender = startPageRender(
          page,
          canvas,
          scale,
          window.devicePixelRatio || 1
        );
        renderTask = startedRender.task;
        await startedRender.task.promise;
      } catch (reason) {
        if (!cancelled) renderError = errorMessage(reason);
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
    observer.observe(host);
    return () => {
      cancelled = true;
      renderTask?.cancel();
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
