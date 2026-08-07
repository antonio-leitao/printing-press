<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import PdfPage from '$lib/PdfPage.svelte';
  import { api, errorMessage } from '$lib/api';
  import { MAX_ZOOM, MIN_ZOOM, normalizeZoom, wheelZoomFactor, zoomBySteps } from '$lib/pdf-controls';
  import { initialKeyState, resolveKey, type KeyState, type ViewerAction } from '$lib/pdf-keys';
  import { PageVisibilityTracker } from '$lib/pdf-visibility';
  import type { ArtifactSummary, PageSize } from '$lib/types';

  let {
    artifact,
    page = $bindable(1),
    pageCount = $bindable(0),
    zoomPercent = $bindable(100),
    loadError = $bindable(''),
    enabled = true,
    onPeek
  } = $props<{
    artifact: ArtifactSummary;
    page?: number;
    pageCount?: number;
    zoomPercent?: number;
    loadError?: string;
    /** False while a dialog owns the keyboard. */
    enabled?: boolean;
    /** Cmd-click, in PDF points from the page's top left. */
    onPeek?: (at: PeekRequest) => void;
  }>();

  export type PeekRequest = {
    page: number;
    x: number;
    y: number;
    /** Where on screen it was asked for, so the answer can be shown there. */
    clientX: number;
    clientY: number;
  };

  type ViewAnchor = {
    page: number;
    pageX: number;
    pageY: number;
    viewportX: number;
    viewportY: number;
  };

  type WebKitGestureEvent = Event & { clientX: number; clientY: number; scale: number };

  type ZoomSession = {
    kind: 'gesture' | 'wheel';
    baseZoom: number;
    targetZoom: number;
    anchor: ViewAnchor | null;
  };

  /** Time constant of the scroll glide. Short enough to feel immediate. */
  const GLIDE_TAU = 55;
  /**
   * A document sits flush against the top and both sides of the window. The
   * only gap anywhere is *between* pages, which is the bottom `margin` on
   * `.page` in `PdfPage.svelte`.
   *
   * Both of these follow from that: nothing is left above a jumped-to page, so
   * `G` lands exactly as page one rests, and a fitted page uses the full width
   * because there are no side gutters to leave room for.
   */
  const PAGE_LEAD = 0;
  const FIT_MARGIN = 0;

  // Only ever replaced once the new document's geometry has arrived; blanking
  // it first is what made every rebuild flash an empty viewer.
  let layout = $state<PageSize[]>([]);
  let shown = $state<ArtifactSummary | null>(null);
  let zoom = $state(1.3);
  let transientScale = $state(1);
  let transformOriginX = $state(0);
  let transformOriginY = $state(0);
  let generation = 0;
  let zoomGeneration = 0;
  let viewer = $state<HTMLDivElement | null>(null);
  let pagesHost = $state<HTMLDivElement | null>(null);
  let tracker = $state<PageVisibilityTracker | null>(null);
  let zoomSession: ZoomSession | null = null;
  let wheelTimer: number | undefined;
  let scrollFrame = 0;
  let keyState: KeyState = initialKeyState();

  /** Whether this viewer has already chosen a zoom for the document it opened. */
  let sized = false;

  let glideX: number | null = null;
  let glideY: number | null = null;
  let glideFrame = 0;
  let glideClock = 0;

  $effect(() => {
    zoomPercent = Math.round(zoom * 100);
  });
  $effect(() => {
    pageCount = layout.length;
  });

  // -- geometry ---------------------------------------------------------

  function pageElements(): HTMLElement[] {
    if (!viewer) return [];
    return Array.from(viewer.querySelectorAll<HTMLElement>('[data-page]'));
  }

  function closestPage(clientY: number): HTMLElement | null {
    let closest: HTMLElement | null = null;
    let closestDistance = Number.POSITIVE_INFINITY;
    for (const element of pageElements()) {
      const rect = element.getBoundingClientRect();
      if (clientY >= rect.top && clientY <= rect.bottom) return element;
      const distance = Math.min(Math.abs(clientY - rect.top), Math.abs(clientY - rect.bottom));
      if (distance < closestDistance) {
        closest = element;
        closestDistance = distance;
      }
    }
    return closest;
  }

  function captureAnchor(clientX?: number, clientY?: number): ViewAnchor | null {
    if (!viewer || layout.length === 0) return null;
    const viewerRect = viewer.getBoundingClientRect();
    const pointX = clientX ?? viewerRect.left + viewer.clientWidth / 2;
    const pointY = clientY ?? viewerRect.top + viewer.clientHeight / 2;
    const element = closestPage(pointY);
    if (!element) return null;
    const pageRect = element.getBoundingClientRect();
    return {
      page: Number(element.dataset.page ?? 1),
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
    if (request !== zoomGeneration || !viewer) return;
    const pageNumber = Math.min(anchor.page, layout.length || anchor.page);
    const element = viewer.querySelector<HTMLElement>(`[data-page="${pageNumber}"]`);
    if (!element) return;
    const viewerRect = viewer.getBoundingClientRect();
    const pageRect = element.getBoundingClientRect();
    const targetX = pageRect.left + pageRect.width * anchor.pageX;
    const targetY = pageRect.top + pageRect.height * anchor.pageY;
    stopGlide();
    viewer.scrollLeft += targetX - (viewerRect.left + anchor.viewportX);
    viewer.scrollTop += targetY - (viewerRect.top + anchor.viewportY);
  }

  // -- scrolling --------------------------------------------------------

  function clampY(value: number) {
    if (!viewer) return 0;
    return Math.max(0, Math.min(value, viewer.scrollHeight - viewer.clientHeight));
  }

  function clampX(value: number) {
    if (!viewer) return 0;
    return Math.max(0, Math.min(value, viewer.scrollWidth - viewer.clientWidth));
  }

  /**
   * Glides toward a target rather than jumping. Repeated keys add to the target
   * that is already in flight, so holding `j` reads as continuous motion instead
   * of a stutter.
   */
  function glideBy(deltaX: number, deltaY: number) {
    if (!viewer) return;
    glideX = clampX((glideX ?? viewer.scrollLeft) + deltaX);
    glideY = clampY((glideY ?? viewer.scrollTop) + deltaY);
    startGlide();
  }

  function glideToY(value: number) {
    if (!viewer) return;
    glideX = glideX ?? viewer.scrollLeft;
    glideY = clampY(value);
    startGlide();
  }

  function stopGlide() {
    glideX = null;
    glideY = null;
    if (glideFrame) cancelAnimationFrame(glideFrame);
    glideFrame = 0;
  }

  function startGlide() {
    if (glideFrame) return;
    glideClock = performance.now();
    const step = (now: number) => {
      glideFrame = 0;
      if (!viewer || glideY === null) return;
      const elapsed = Math.min(64, now - glideClock);
      glideClock = now;
      // Frame-rate independent exponential approach.
      const progress = 1 - Math.exp(-elapsed / GLIDE_TAU);
      const remainingY = glideY - viewer.scrollTop;
      const remainingX = (glideX ?? viewer.scrollLeft) - viewer.scrollLeft;
      if (Math.abs(remainingY) < 0.5 && Math.abs(remainingX) < 0.5) {
        viewer.scrollTop = glideY;
        if (glideX !== null) viewer.scrollLeft = glideX;
        stopGlide();
        return;
      }
      viewer.scrollTop += remainingY * progress;
      viewer.scrollLeft += remainingX * progress;
      glideFrame = requestAnimationFrame(step);
    };
    glideFrame = requestAnimationFrame(step);
  }

  function pageTop(pageNumber: number): number | null {
    if (!viewer) return null;
    const element = viewer.querySelector<HTMLElement>(`[data-page="${pageNumber}"]`);
    if (!element) return null;
    return (
      element.getBoundingClientRect().top -
      viewer.getBoundingClientRect().top +
      viewer.scrollTop -
      PAGE_LEAD
    );
  }

  function goToPage(pageNumber: number) {
    if (layout.length === 0) return;
    const clamped = Math.max(1, Math.min(pageNumber, layout.length));
    const top = pageTop(clamped);
    if (top === null) {
      // The page is not laid out yet; approximate from the average page height.
      if (!viewer) return;
      const total = viewer.scrollHeight - viewer.clientHeight;
      glideToY((total * (clamped - 1)) / Math.max(1, layout.length - 1));
      return;
    }
    glideToY(top);
  }

  // -- zoom -------------------------------------------------------------

  async function commitZoom(value: number, anchor = captureAnchor()) {
    const nextZoom = normalizeZoom(value);
    transientScale = 1;
    zoomSession = null;
    if (nextZoom === zoom) return;
    zoom = nextZoom;
    const request = ++zoomGeneration;
    await restoreAnchor(anchor, request);
  }

  function startZoomSession(kind: ZoomSession['kind'], clientX: number, clientY: number) {
    transientScale = 1;
    const hostRect = (pagesHost ?? viewer)?.getBoundingClientRect();
    transformOriginX = clientX - (hostRect?.left ?? 0);
    transformOriginY = clientY - (hostRect?.top ?? 0);
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

  /** A wheel delta in pixels, whatever unit the device reported it in. */
  function wheelPixels(delta: number, event: WheelEvent, extent: number): number {
    if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) return delta * 16;
    if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) return delta * extent;
    return delta;
  }

  function wheelDeltaPixels(event: WheelEvent): number {
    return wheelPixels(event.deltaY, event, viewer?.clientHeight ?? 0);
  }

  /**
   * Panning sideways, which the webview will not do on its own: a horizontal
   * trackpad swipe is claimed as a back-forward navigation gesture before the
   * page ever sees it as scrolling.
   *
   * Only taken when there is somewhere to pan to, so an unzoomed document
   * scrolls natively as before. Once the event is claimed, its vertical part
   * has to be applied here too, or a diagonal swipe would lose it.
   */
  function panSideways(event: WheelEvent): boolean {
    if (!viewer) return false;
    if (viewer.scrollWidth - viewer.clientWidth <= 1) return false;
    // Shift turns a vertical wheel into a horizontal one, as it does elsewhere.
    const sideways = event.deltaX !== 0 ? event.deltaX : event.shiftKey ? event.deltaY : 0;
    if (sideways === 0) return false;

    event.preventDefault();
    viewer.scrollLeft += wheelPixels(sideways, event, viewer.clientWidth);
    if (event.deltaX !== 0 && event.deltaY !== 0) {
      viewer.scrollTop += wheelPixels(event.deltaY, event, viewer.clientHeight);
    }
    return true;
  }

  function handleWheel(event: WheelEvent) {
    if (!event.ctrlKey && !event.metaKey) {
      // A real scroll takes over from any glide in flight.
      stopGlide();
      panSideways(event);
      return;
    }
    if (layout.length === 0) return;
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
    if (layout.length === 0) return;
    event.preventDefault();
    stopGlide();
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

  async function fitTo(mode: 'actual' | 'page' | 'width') {
    if (mode === 'actual') {
      await commitZoom(1);
      return;
    }
    if (!viewer || layout.length === 0) return;
    const anchor = captureAnchor();
    const size = layout[Math.min(anchor?.page ?? page, layout.length) - 1];
    if (!size) return;
    const scale =
      mode === 'width'
        ? Math.max(1, viewer.clientWidth - FIT_MARGIN) / size.width
        : Math.max(1, viewer.clientHeight - FIT_MARGIN) / size.height;
    await commitZoom(scale, anchor);
  }

  /**
   * A document opens with its first page filling the height of the window, at
   * the top of that page — so what you see on open is a page, not the top of
   * one. Only ever on open: a rebuild, or switching to a stored version, must
   * not override a zoom the reader chose or scroll them away from where they
   * were reading.
   */
  async function fitHeightOnOpen(sizes: PageSize[]) {
    await tick();
    // A frame after the DOM update, because the viewer has no measurable size
    // until it has been laid out at least once.
    await nextFrame();
    if (!viewer || sizes.length === 0) return;
    const available = viewer.clientHeight - FIT_MARGIN;
    // Still not laid out. Leaving `sized` false means the next document to
    // arrive tries again rather than being stuck at the default zoom.
    if (available <= 0) return;
    zoom = normalizeZoom(available / sizes[0].height);
    sized = true;

    // After the new zoom has been laid out, or the reset would race the taller
    // content and leave the document a little way down.
    await tick();
    if (!viewer) return;
    stopGlide();
    viewer.scrollTop = 0;
    viewer.scrollLeft = 0;
  }

  // -- keyboard ---------------------------------------------------------

  function editableTarget(target: EventTarget | null): boolean {
    return (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      target instanceof HTMLSelectElement ||
      (target instanceof HTMLElement && target.isContentEditable)
    );
  }

  function scrollAmount(amount: 'step' | 'half' | 'page', axis: 'x' | 'y'): number {
    if (!viewer) return 0;
    const extent = axis === 'y' ? viewer.clientHeight : viewer.clientWidth;
    if (amount === 'half') return extent * 0.5;
    // A page keeps a sliver of context rather than jumping cleanly.
    if (amount === 'page') return extent * 0.92;
    return Math.max(56, extent * 0.1);
  }

  function runAction(action: ViewerAction) {
    switch (action.kind) {
      case 'scroll': {
        const distance = scrollAmount(action.amount, action.axis) * action.sign * action.count;
        glideBy(action.axis === 'x' ? distance : 0, action.axis === 'y' ? distance : 0);
        return;
      }
      case 'goto':
        if (action.target === 'first') glideToY(0);
        else if (action.target === 'last') glideToY(Number.MAX_SAFE_INTEGER);
        else if (action.page !== undefined) goToPage(action.page);
        return;
      case 'page':
        goToPage(page + action.sign * action.count);
        return;
      case 'zoom':
        void commitZoom(zoomBySteps(zoom, action.step));
        return;
      case 'fit':
        void fitTo(action.mode);
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (!enabled || layout.length === 0 || editableTarget(event.target)) return;

    // The platform zoom chords stay on Command, separate from the vim keys.
    if (event.metaKey && !event.ctrlKey && !event.altKey) {
      const plus = event.key === '+' || event.key === '=' || event.code === 'NumpadAdd';
      const minus = event.key === '-' || event.code === 'NumpadSubtract';
      if (plus || minus) {
        event.preventDefault();
        void commitZoom(zoomBySteps(zoom, plus ? 1 : -1));
      } else if (event.key === '0') {
        event.preventDefault();
        void fitTo('actual');
      } else if (event.key === '9') {
        event.preventDefault();
        void fitTo('width');
      }
      return;
    }

    const { resolution, state } = resolveKey(event, keyState);
    keyState = state;
    if (resolution.kind === 'ignored') return;
    event.preventDefault();
    if (resolution.kind === 'action') runAction(resolution.action);
  }

  // -- lifecycle --------------------------------------------------------

  function updateCurrentPage() {
    scrollFrame = 0;
    if (!viewer) return;
    const viewerRect = viewer.getBoundingClientRect();
    const centre = viewerRect.top + viewer.clientHeight / 2;
    const element = closestPage(centre);
    if (element) page = Number(element.dataset.page ?? 1);
  }

  function handleScroll() {
    if (!scrollFrame) scrollFrame = requestAnimationFrame(updateCurrentPage);
  }

  /**
   * Cmd-click asks what made this. The point is reported in PDF points from the
   * page's top left, which is the only thing outside this component that means
   * anything: the zoom, the transient scale during a pinch and the device pixel
   * ratio are all this viewer's business.
   *
   * Cmd rather than double-click, which belongs to selecting a word.
   */
  function handleClick(event: MouseEvent) {
    if (!onPeek || !(event.metaKey || event.ctrlKey)) return;
    const target = event.target as HTMLElement | null;
    const element = target?.closest<HTMLElement>('[data-page]');
    if (!element) return;
    const pageNumber = Number(element.dataset.page ?? 0);
    const size = layout[pageNumber - 1];
    if (!size) return;

    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    event.preventDefault();
    onPeek({
      page: pageNumber,
      x: ((event.clientX - rect.left) / rect.width) * size.width,
      y: ((event.clientY - rect.top) / rect.height) * size.height,
      clientX: event.clientX,
      clientY: event.clientY
    });
  }

  onMount(() => {
    const element = viewer;
    if (!element) return;
    tracker = new PageVisibilityTracker(element);
    element.addEventListener('wheel', handleWheel, { passive: false });
    element.addEventListener('gesturestart', handleGestureStart, { passive: false });
    element.addEventListener('gesturechange', handleGestureChange, { passive: false });
    element.addEventListener('gestureend', handleGestureEnd, { passive: false });
    element.addEventListener('scroll', handleScroll, { passive: true });
    element.addEventListener('pointerdown', stopGlide);
    element.addEventListener('click', handleClick);
    window.addEventListener('keydown', handleKeydown);
    return () => {
      element.removeEventListener('wheel', handleWheel);
      element.removeEventListener('gesturestart', handleGestureStart);
      element.removeEventListener('gesturechange', handleGestureChange);
      element.removeEventListener('gestureend', handleGestureEnd);
      element.removeEventListener('scroll', handleScroll);
      element.removeEventListener('pointerdown', stopGlide);
      element.removeEventListener('click', handleClick);
      window.removeEventListener('keydown', handleKeydown);
      if (wheelTimer !== undefined) window.clearTimeout(wheelTimer);
      if (scrollFrame) cancelAnimationFrame(scrollFrame);
      stopGlide();
      tracker?.disconnect();
      tracker = null;
    };
  });

  $effect(() => {
    const next = artifact;
    const request = ++generation;

    // Geometry for a whole document costs a few milliseconds, so the layout is
    // known before any page is drawn and the page counter is right immediately.
    void api
      .pageLayout(next.id)
      .then((sizes) => {
        if (request !== generation) return;
        // Captured from the document still on screen, then reapplied to the new
        // one, so a rebuild leaves the reader where they were.
        const anchor = untrack(captureAnchor);
        layout = sizes;
        shown = next;
        loadError = '';
        if (sized) {
          void restoreAnchor(anchor, ++zoomGeneration);
        } else {
          void fitHeightOnOpen(sizes);
        }
      })
      .catch((reason) => {
        if (request !== generation) return;
        // The previous document stays on screen; taking it away is the worst
        // thing the viewer can do.
        loadError = errorMessage(reason);
      });
  });
</script>

<div class="viewer" bind:this={viewer} tabindex="-1">
  {#if shown && tracker && layout.length > 0}
    <div
      class="pages"
      class:zooming={transientScale !== 1}
      bind:this={pagesHost}
      style:transform={`scale(${transientScale})`}
      style:transform-origin={`${transformOriginX}px ${transformOriginY}px`}
    >
      {#each layout as size, index}
        <PdfPage
          artifact={shown}
          size={size}
          pageNumber={index + 1}
          zoom={zoom}
          tracker={tracker}
        />
      {/each}
    </div>
  {:else if !loadError}
    <p class="placeholder">Opening PDF…</p>
  {/if}
</div>

<style>
  .viewer {
    height: 100%;
    min-height: 0;
    overflow: auto;
    outline: none;
    overscroll-behavior: contain;
    background: var(--paper);
  }

  .pages {
    /* flow-root so the last page's bottom margin stays inside the scrollable
       area instead of being dropped at the end of the document. Nothing else
       adds spacing here: the first page starts at y=0. */
    display: flow-root;
    width: max-content;
    min-width: 100%;
    padding: 0;
  }

  .pages.zooming {
    will-change: transform;
  }

  .placeholder {
    margin: 2rem;
    /* Dark, because the viewer's background is now nearly white. */
    color: var(--ink-3);
  }
</style>
