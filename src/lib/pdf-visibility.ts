/**
 * Two shared observers for every page in a document.
 *
 * One per page would mean four hundred observers in a thesis. More importantly,
 * the two bands give hysteresis: a page starts rendering when it comes within
 * `RENDER_MARGIN`, and only gives its bitmap back once it is past the much
 * larger `RETAIN_MARGIN`. Without that gap, scrolling along the boundary would
 * render and discard the same page repeatedly.
 */

/** Render this far outside the viewport, so scrolling meets a drawn page. */
const RENDER_MARGIN = '800px 0px';
/** Keep the bitmap this far outside, so a small scroll back costs nothing. */
const RETAIN_MARGIN = '2400px 0px';

export type PageVisibility = {
  /** Close enough that this page should be drawn. */
  render: boolean;
  /** Close enough that a drawn page should keep its bitmap. */
  retain: boolean;
};

type Entry = {
  visibility: PageVisibility;
  notify: (visibility: PageVisibility) => void;
};

export class PageVisibilityTracker {
  #entries = new Map<Element, Entry>();
  #render: IntersectionObserver;
  #retain: IntersectionObserver;

  constructor(root: Element | null) {
    this.#render = new IntersectionObserver(
      (records) => this.#apply(records, 'render'),
      { root, rootMargin: RENDER_MARGIN }
    );
    this.#retain = new IntersectionObserver(
      (records) => this.#apply(records, 'retain'),
      { root, rootMargin: RETAIN_MARGIN }
    );
  }

  observe(element: Element, notify: (visibility: PageVisibility) => void) {
    this.#entries.set(element, {
      visibility: { render: false, retain: false },
      notify
    });
    this.#render.observe(element);
    this.#retain.observe(element);
  }

  unobserve(element: Element) {
    this.#entries.delete(element);
    this.#render.unobserve(element);
    this.#retain.unobserve(element);
  }

  disconnect() {
    this.#entries.clear();
    this.#render.disconnect();
    this.#retain.disconnect();
  }

  #apply(records: IntersectionObserverEntry[], band: keyof PageVisibility) {
    for (const record of records) {
      const entry = this.#entries.get(record.target);
      if (!entry || entry.visibility[band] === record.isIntersecting) continue;
      entry.visibility = { ...entry.visibility, [band]: record.isIntersecting };
      entry.notify(entry.visibility);
    }
  }
}
