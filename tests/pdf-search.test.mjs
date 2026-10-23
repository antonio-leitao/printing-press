import assert from 'node:assert/strict';
import test from 'node:test';
import { describe, firstFrom, groupByPage, hitsOn, stepFrom } from '../src/lib/pdf-search.ts';

function hit(page, y, x = 72) {
  return { page, x, y, width: 30, height: 10 };
}

/** In reading order, the way the renderer returns them. */
const hits = [hit(0, 100), hit(0, 400), hit(2, 50), hit(2, 300), hit(5, 700)];

test('hits are grouped by the page they are on', () => {
  const pages = groupByPage(hits);
  assert.equal(hitsOn(pages, 0).length, 2);
  assert.equal(hitsOn(pages, 2).length, 2);
  assert.equal(hitsOn(pages, 5).length, 1);
  assert.deepEqual(hitsOn(pages, 1), []);
  // Every empty page shares one array, so clearing a search re-renders nothing.
  assert.equal(hitsOn(pages, 1), hitsOn(pages, 3));
});

test('a search lands forward of where the reader is', () => {
  assert.equal(firstFrom(hits, 0, 0), 0);
  assert.equal(firstFrom(hits, 0, 101), 1);
  // Between hits on a page, and on a page with none.
  assert.equal(firstFrom(hits, 1, 0), 2);
  assert.equal(firstFrom(hits, 2, 200), 3);
  assert.equal(firstFrom(hits, 3, 0), 4);
});

test('past the last hit, a search wraps to the first', () => {
  assert.equal(firstFrom(hits, 5, 701), 0);
  assert.equal(firstFrom(hits, 9, 0), 0);
  assert.equal(firstFrom([], 0, 0), -1);
});

test('n and N wrap at both ends', () => {
  assert.equal(stepFrom(0, 5, 1), 1);
  assert.equal(stepFrom(4, 5, 1), 0);
  assert.equal(stepFrom(0, 5, -1), 4);
  assert.equal(stepFrom(1, 5, 1, 7), 3);
  assert.equal(stepFrom(1, 5, -1, 7), 4);
  assert.equal(stepFrom(2, 0, 1), -1);
});

test('with nothing chosen yet, n starts at the first hit and N at the last', () => {
  assert.equal(stepFrom(-1, 5, 1), 0);
  assert.equal(stepFrom(-1, 5, -1), 4);
});

test('the prompt counts from one', () => {
  assert.equal(describe(0, 5), '1 of 5');
  assert.equal(describe(4, 5), '5 of 5');
  assert.equal(describe(-1, 0), 'No matches');
});
