import assert from 'node:assert/strict';
import test from 'node:test';

/** Records every observer the tracker creates, and lets tests fire records. */
class FakeIntersectionObserver {
  static instances = [];

  constructor(callback, options) {
    this.callback = callback;
    this.options = options;
    this.observed = new Set();
    FakeIntersectionObserver.instances.push(this);
  }

  observe(element) {
    this.observed.add(element);
  }

  unobserve(element) {
    this.observed.delete(element);
  }

  disconnect() {
    this.observed.clear();
  }

  fire(target, isIntersecting) {
    this.callback([{ target, isIntersecting }]);
  }
}

globalThis.IntersectionObserver = FakeIntersectionObserver;

const { PageVisibilityTracker } = await import('../src/lib/pdf-visibility.ts');

function setup() {
  FakeIntersectionObserver.instances = [];
  const tracker = new PageVisibilityTracker(null);
  const [render, retain] = FakeIntersectionObserver.instances;
  return { tracker, render, retain };
}

function margin(observer) {
  return Number.parseInt(observer.options.rootMargin, 10);
}

test('uses two shared observers rather than one per page', () => {
  const { tracker, render, retain } = setup();
  const pages = [{ id: 1 }, { id: 2 }, { id: 3 }];
  for (const page of pages) tracker.observe(page, () => {});

  assert.equal(FakeIntersectionObserver.instances.length, 2);
  assert.equal(render.observed.size, 3);
  assert.equal(retain.observed.size, 3);
});

test('keeps a bitmap after a page stops being worth drawing', () => {
  const { tracker, render, retain } = setup();
  const page = { id: 'page-1' };
  const seen = [];
  tracker.observe(page, (visibility) => seen.push({ ...visibility }));

  render.fire(page, true);
  retain.fire(page, true);
  assert.deepEqual(seen.at(-1), { render: true, retain: true });

  // Scrolled past the render band: stop drawing, but the canvas is still worth
  // keeping so a small scroll back is free.
  render.fire(page, false);
  assert.deepEqual(seen.at(-1), { render: false, retain: true });

  // Only once it leaves the outer band does the bitmap become disposable.
  retain.fire(page, false);
  assert.deepEqual(seen.at(-1), { render: false, retain: false });
});

test('the retain band is strictly larger than the render band', () => {
  const { render, retain } = setup();
  assert.ok(
    margin(retain) > margin(render),
    'without a gap between the bands, scrolling on the boundary would thrash'
  );
});

test('reports only actual changes', () => {
  const { tracker, render } = setup();
  const page = { id: 'page-1' };
  let notifications = 0;
  tracker.observe(page, () => (notifications += 1));

  render.fire(page, true);
  render.fire(page, true);
  render.fire(page, true);
  assert.equal(notifications, 1);

  render.fire(page, false);
  assert.equal(notifications, 2);
});

test('an unobserved page stops being tracked', () => {
  const { tracker, render, retain } = setup();
  const page = { id: 'page-1' };
  let notifications = 0;
  tracker.observe(page, () => (notifications += 1));
  tracker.unobserve(page);

  assert.equal(render.observed.size, 0);
  assert.equal(retain.observed.size, 0);
  render.fire(page, true);
  assert.equal(notifications, 0);
});

test('disconnect drops every registration', () => {
  const { tracker, render, retain } = setup();
  const page = { id: 'page-1' };
  let notifications = 0;
  tracker.observe(page, () => (notifications += 1));
  tracker.disconnect();

  assert.equal(render.observed.size, 0);
  assert.equal(retain.observed.size, 0);
  render.fire(page, true);
  assert.equal(notifications, 0);
});
