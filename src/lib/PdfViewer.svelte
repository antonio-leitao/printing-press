<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import type { PDFDocumentProxy } from 'pdfjs-dist';
  import PdfPage from '$lib/PdfPage.svelte';
  import { errorMessage } from '$lib/api';
  import {
    MAX_ZOOM,
    MIN_ZOOM,
    normalizeZoom,
    wheelZoomFactor,
    zoomBySteps
  } from '$lib/pdf-controls';
  import { loadProjectPdf } from '$lib/pdf';

  let { projectId, revision } = $props<{
    projectId: number;
    revision: number;
  }>();

  type ViewAnchor = {
    page: number;
    pageX: number;
    pageY: number;
    viewportX: number;
    viewportY: number;
  };

  type WebKitGestureEvent = Event & {
    clientX: number;
    clientY: number;
    scale: number;
  };

  type ZoomSession = {
    kind: 'gesture' | 'wheel';
    baseZoom: number;
    targetZoom: number;
    anchor: ViewAnchor | null;
  };

  let document = $state<PDFDocumentProxy | null>(null);
  let error = $state('');
  let zoom = $state(1.3);
  let currentPage = $state(1);
  let transientScale = $state(1);
  let transformOriginX = $state(0);
  let transformOriginY = $state(0);
  let generation = 0;
  let zoomGeneration = 0;
  let viewer: HTMLDivElement;
  let pagesHost = $state<HTMLDivElement | null>(null);
  let zoomSession: ZoomSession | null = null;
  let wheelTimer: number | undefined;
  let scrollFrame = 0;

  const zoomPercent = $derived(Math.round(zoom * 100));

  function pageElements(): HTMLElement[] {
    if (!viewer) return [];
    return Array.from(viewer.querySelectorAll<HTMLElement>('[data-page]'));
  }

  function closestPage(clientY: number): HTMLElement | null {
    let closest: HTMLElement | null = null;
    let closestDistance = Number.POSITIVE_INFINITY;
    for (const page of pageElements()) {
      const rect = page.getBoundingClientRect();
      if (clientY >= rect.top && clientY <= rect.bottom) return page;
      const distance = Math.min(Math.abs(clientY - rect.top), Math.abs(clientY - rect.bottom));
      if (distance < closestDistance) {
        closest = page;
        closestDistance = distance;
      }
    }
    return closest;
  }

  function captureAnchor(clientX?: number, clientY?: number): ViewAnchor | null {
    if (!viewer || !document) return null;
    const viewerRect = viewer.getBoundingClientRect();
    const pointX = clientX ?? viewerRect.left + viewer.clientWidth / 2;
    const pointY = clientY ?? viewerRect.top + viewer.clientHeight / 2;
    const page = closestPage(pointY);
    if (!page) return null;
    const pageRect = page.getBoundingClientRect();
    return {
      page: Number(page.dataset.page ?? 1),
      pageX: Math.min(1, Math.max(0, (pointX - pageRect.left) / Math.max(1, pageRect.width))),
      pageY: Math.min(1, Math.max(0, (pointY - pageRect.top) / Math.max(1, pageRect.height))),
      viewportX: pointX - viewerRect.left,
      viewportY: pointY - viewerRect.top
    };
  }

  async function nextFrame() {
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  }

  async function restoreAnchor(anchor: ViewAnchor | null, request: number) {
    if (!anchor || !viewer) return;
    await tick();
    await nextFrame();
    await nextFrame();
    if (request !== zoomGeneration) return;
    const pageNumber = Math.min(anchor.page, document?.numPages ?? anchor.page);
    const page = viewer.querySelector<HTMLElement>(`[data-page="${pageNumber}"]`);
    if (!page) return;
    const viewerRect = viewer.getBoundingClientRect();
    const pageRect = page.getBoundingClientRect();
    const targetX = pageRect.left + pageRect.width * anchor.pageX;
    const targetY = pageRect.top + pageRect.height * anchor.pageY;
    viewer.scrollLeft += targetX - (viewerRect.left + anchor.viewportX);
    viewer.scrollTop += targetY - (viewerRect.top + anchor.viewportY);
  }

  async function commitZoom(value: number, anchor = captureAnchor()) {
    const nextZoom = normalizeZoom(value);
    transientScale = 1;
    zoomSession = null;
    if (nextZoom === zoom) return;
    zoom = nextZoom;
    const request = ++zoomGeneration;
    await restoreAnchor(anchor, request);
  }

  function startZoomSession(
    kind: ZoomSession['kind'],
    clientX: number,
    clientY: number
  ): ZoomSession {
    transientScale = 1;
    const hostRect = (pagesHost ?? viewer).getBoundingClientRect();
    transformOriginX = clientX - hostRect.left;
    transformOriginY = clientY - hostRect.top;
    const session = {
      kind,
      baseZoom: zoom,
      targetZoom: zoom,
      anchor: captureAnchor(clientX, clientY)
    } satisfies ZoomSession;
    zoomSession = session;
    return session;
  }

  function previewZoom(session: ZoomSession, target: number) {
    session.targetZoom = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, target));
    transientScale = session.targetZoom / session.baseZoom;
  }

  function finishZoomSession(kind: ZoomSession['kind']) {
    const session = zoomSession;
    if (!session || session.kind !== kind) return;
    zoomSession = null;
    void commitZoom(session.targetZoom, session.anchor);
  }

  function wheelDeltaPixels(event: WheelEvent): number {
    if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) return event.deltaY * 16;
    if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) return event.deltaY * viewer.clientHeight;
    return event.deltaY;
  }

  function handleWheel(event: WheelEvent) {
    if ((!event.ctrlKey && !event.metaKey) || !document) return;
    event.preventDefault();
    if (zoomSession?.kind === 'gesture') return;
    const session =
      zoomSession?.kind === 'wheel'
        ? zoomSession
        : startZoomSession('wheel', event.clientX, event.clientY);
    previewZoom(session, session.targetZoom * wheelZoomFactor(wheelDeltaPixels(event)));
    if (wheelTimer !== undefined) window.clearTimeout(wheelTimer);
    wheelTimer = window.setTimeout(() => finishZoomSession('wheel'), 120);
  }

  function handleGestureStart(event: Event) {
    if (!document) return;
    event.preventDefault();
    if (wheelTimer !== undefined) window.clearTimeout(wheelTimer);
    const gesture = event as WebKitGestureEvent;
    startZoomSession('gesture', gesture.clientX, gesture.clientY);
  }

  function handleGestureChange(event: Event) {
    const session = zoomSession;
    if (!session || session.kind !== 'gesture') return;
    event.preventDefault();
    previewZoom(session, session.baseZoom * (event as WebKitGestureEvent).scale);
  }

  function handleGestureEnd(event: Event) {
    event.preventDefault();
    finishZoomSession('gesture');
  }

  function changeZoom(steps: number) {
    void commitZoom(zoomBySteps(zoom, steps));
  }

  function actualSize() {
    void commitZoom(1);
  }

  async function fitWidth() {
    if (!document || !viewer) return;
    const anchor = captureAnchor();
    const pageNumber = Math.min(anchor?.page ?? currentPage, document.numPages);
    const page = await document.getPage(pageNumber);
    const viewport = page.getViewport({ scale: 1 });
    const availableWidth = Math.max(1, viewer.clientWidth - 48);
    await commitZoom(availableWidth / viewport.width, anchor);
  }

  function applyZoomInput(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const percentage = Number(input.value);
    if (Number.isFinite(percentage)) void commitZoom(percentage / 100);
    input.value = String(zoomPercent);
  }

  function editableTarget(target: EventTarget | null): boolean {
    return (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      target instanceof HTMLSelectElement ||
      (target instanceof HTMLElement && target.isContentEditable)
    );
  }

  function handleKeydown(event: KeyboardEvent) {
    const command = event.metaKey || event.ctrlKey;
    if (!command || !document) return;
    const plus = event.key === '+' || event.key === '=' || event.code === 'NumpadAdd';
    const minus = event.key === '-' || event.code === 'NumpadSubtract';
    if (plus) {
      event.preventDefault();
      changeZoom(1);
    } else if (minus) {
      event.preventDefault();
      changeZoom(-1);
    } else if (event.key === '0') {
      event.preventDefault();
      actualSize();
    } else if (event.key === '9') {
      event.preventDefault();
      void fitWidth();
    } else if (editableTarget(event.target)) {
      return;
    }
  }

  function updateCurrentPage() {
    scrollFrame = 0;
    if (!viewer) return;
    const viewerRect = viewer.getBoundingClientRect();
    const center = viewerRect.top + viewer.clientHeight / 2;
    const page = closestPage(center);
    if (page) currentPage = Number(page.dataset.page ?? 1);
  }

  function handleScroll() {
    if (!scrollFrame) scrollFrame = requestAnimationFrame(updateCurrentPage);
  }

  onMount(() => {
    viewer.addEventListener('wheel', handleWheel, { passive: false });
    viewer.addEventListener('gesturestart', handleGestureStart, { passive: false });
    viewer.addEventListener('gesturechange', handleGestureChange, { passive: false });
    viewer.addEventListener('gestureend', handleGestureEnd, { passive: false });
    viewer.addEventListener('scroll', handleScroll, { passive: true });
    window.addEventListener('keydown', handleKeydown);
    return () => {
      viewer.removeEventListener('wheel', handleWheel);
      viewer.removeEventListener('gesturestart', handleGestureStart);
      viewer.removeEventListener('gesturechange', handleGestureChange);
      viewer.removeEventListener('gestureend', handleGestureEnd);
      viewer.removeEventListener('scroll', handleScroll);
      window.removeEventListener('keydown', handleKeydown);
      if (wheelTimer !== undefined) window.clearTimeout(wheelTimer);
      if (scrollFrame) cancelAnimationFrame(scrollFrame);
    };
  });

  $effect(() => {
    const currentProject = projectId;
    const currentRevision = revision;
    const currentGeneration = ++generation;
    const anchor = untrack(captureAnchor);
    document = null;
    error = '';
    void loadProjectPdf(currentProject, currentRevision)
      .then((loaded) => {
        if (currentGeneration === generation) {
          document = loaded;
          const request = ++zoomGeneration;
          void restoreAnchor(anchor, request);
        }
      })
      .catch((reason) => {
        if (currentGeneration === generation) error = errorMessage(reason);
      });
  });
</script>

<div class="viewer-shell">
  <nav class="viewer-toolbar" aria-label="PDF controls">
    <button onclick={() => changeZoom(-1)} disabled={zoom <= MIN_ZOOM} title="Zoom out (⌘−)">
      −
    </button>
    <label class="zoom-field">
      <span class="visually-hidden">Zoom percentage</span>
      <input
        type="number"
        min={MIN_ZOOM * 100}
        max={MAX_ZOOM * 100}
        step="10"
        value={zoomPercent}
        onchange={applyZoomInput}
        aria-label="Zoom percentage"
      />
      <span>%</span>
    </label>
    <button onclick={() => changeZoom(1)} disabled={zoom >= MAX_ZOOM} title="Zoom in (⌘+)">
      +
    </button>
    <button onclick={actualSize} title="Actual size (⌘0)">Actual Size</button>
    <button onclick={fitWidth} title="Fit width (⌘9)">Fit Width</button>
    {#if document}<span class="page-status">Page {currentPage} of {document.numPages}</span>{/if}
  </nav>

  <div class="viewer" bind:this={viewer}>
    {#if error}
      <p role="alert">Could not open the cached PDF: {error}</p>
    {:else if !document}
      <p>Opening PDF…</p>
    {:else}
      <div
        class="pages"
        class:zooming={transientScale !== 1}
        bind:this={pagesHost}
        style:transform={`scale(${transientScale})`}
        style:transform-origin={`${transformOriginX}px ${transformOriginY}px`}
      >
        {#each Array.from({ length: document.numPages }, (_, index) => index + 1) as page}
          <PdfPage {document} pageNumber={page} scale={zoom} />
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .viewer-shell {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    height: 100%;
    min-height: 0;
  }

  .viewer-toolbar {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    padding: 0.4rem 0.75rem;
    border-bottom: 1px solid #999;
    background: #eee;
  }

  .viewer-toolbar button {
    padding: 0.3rem 0.55rem;
  }

  .zoom-field {
    display: flex;
    align-items: center;
    gap: 0.2rem;
  }

  .zoom-field input {
    width: 4.5rem;
    font: inherit;
    text-align: right;
  }

  .page-status {
    margin-left: auto;
    white-space: nowrap;
  }

  .viewer {
    height: 100%;
    min-height: 0;
    overflow: auto;
    overscroll-behavior: contain;
    background: #b7b7b7;
  }

  .pages {
    width: max-content;
    min-width: 100%;
    padding: 1px 16px 16px;
  }

  .pages.zooming {
    will-change: transform;
  }

  .viewer > p {
    margin: 2rem;
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
