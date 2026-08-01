<script lang="ts">
  import { onMount } from 'svelte';
  import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist';
  import { errorMessage } from '$lib/api';
  import { startPageRender } from '$lib/pdf-render';

  let { document, pageNumber, scale = 1.3 } = $props<{
    document: PDFDocumentProxy;
    pageNumber: number;
    scale?: number;
  }>();

  let host: HTMLElement;
  let canvas: HTMLCanvasElement;
  let baseWidth = $state(612);
  let baseHeight = $state(792);
  let isNear = $state(false);
  let renderError = $state('');
  let renderedScale: number | null = null;
  const width = $derived(baseWidth * scale);
  const height = $derived(baseHeight * scale);

  onMount(() => {
    const observer = new IntersectionObserver(
      (entries) => {
        isNear = entries.some((entry) => entry.isIntersecting);
      },
      { rootMargin: '800px 0px' }
    );
    observer.observe(host);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (!isNear) return;
    const currentDocument = document;
    const currentPage = pageNumber;
    const currentScale = scale;
    if (renderedScale === currentScale) return;

    let cancelled = false;
    let renderTask: RenderTask | undefined;
    renderError = '';
    void (async () => {
      try {
        const page = await currentDocument.getPage(currentPage);
        if (cancelled) return;
        const naturalViewport = page.getViewport({ scale: 1 });
        baseWidth = naturalViewport.width;
        baseHeight = naturalViewport.height;
        const startedRender = startPageRender(
          page,
          canvas,
          currentScale,
          window.devicePixelRatio || 1
        );
        renderTask = startedRender.task;
        await startedRender.task.promise;
        if (!cancelled) renderedScale = currentScale;
      } catch (reason) {
        if (!cancelled) renderError = errorMessage(reason);
      }
    })();

    return () => {
      cancelled = true;
      renderTask?.cancel();
    };
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
  .page {
    margin: 16px auto;
    background: white;
    box-shadow: 0 1px 5px #0005;
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
