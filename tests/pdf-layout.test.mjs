import assert from 'node:assert/strict';
import test from 'node:test';
import { indexAt } from '../src/lib/pdf-layout.ts';

/** A column of pages of mixed height, with a gap between them. */
function column(heights, gap = 16) {
  const tops = [];
  let top = 0;
  for (const height of heights) {
    tops.push(top);
    top += height + gap;
  }
  return tops;
}

function pageAt(tops, offset) {
  return indexAt(tops.length, offset, (index) => tops[index]);
}

test('finds the page an offset lands in', () => {
  const tops = column([800, 800, 800, 800]);
  assert.equal(pageAt(tops, 0), 0);
  assert.equal(pageAt(tops, 799), 0);
  assert.equal(pageAt(tops, 816), 1);
  assert.equal(pageAt(tops, 1000), 1);
  assert.equal(pageAt(tops, 2448), 3);
});

test('pages of different heights are found by position, not by index', () => {
  // A landscape plate between upright pages: dividing the document evenly
  // would put every offset past it on the wrong page.
  const tops = column([800, 400, 1200, 800]);
  assert.equal(pageAt(tops, 810), 0, 'still in the gap under page one');
  assert.equal(pageAt(tops, 816), 1);
  assert.equal(pageAt(tops, 1231), 1);
  assert.equal(pageAt(tops, 1232), 2);
  assert.equal(pageAt(tops, 2447), 2);
  assert.equal(pageAt(tops, 2448), 3);
});

test('an offset above the document is the first page', () => {
  const tops = column([800, 800]);
  // Overscrolling upwards is still looking at page one.
  assert.equal(pageAt(tops, -400), 0);
});

test('an offset past the end is the last page', () => {
  const tops = column([800, 800]);
  assert.equal(pageAt(tops, 99_999), 1);
});

test('nothing to search is reported rather than guessed at', () => {
  assert.equal(indexAt(0, 100, () => 0), -1);
  assert.equal(indexAt(-1, 100, () => 0), -1);
});

test('a single page answers every offset', () => {
  const tops = column([800]);
  for (const offset of [-10, 0, 400, 10_000]) {
    assert.equal(pageAt(tops, offset), 0);
  }
});

/// The whole reason for the binary search: a walk measures every page above the
/// one being read, on every scroll frame.
test('measures a handful of pages rather than all of them', () => {
  const tops = column(Array.from({ length: 512 }, () => 800));
  let measured = 0;
  const found = indexAt(tops.length, 500 * 816 + 40, (index) => {
    measured += 1;
    return tops[index];
  });
  assert.equal(found, 500);
  assert.ok(measured <= 10, `measured ${measured} of 512 pages`);
});

test('agrees with a walk over the whole column', () => {
  const heights = [800, 400, 1200, 640, 900, 1100, 300];
  const tops = column(heights);
  const walk = (offset) => {
    let last = 0;
    for (let index = 0; index < tops.length; index += 1) {
      if (tops[index] <= offset) last = index;
    }
    return last;
  };
  for (let offset = 0; offset < 6000; offset += 7) {
    assert.equal(pageAt(tops, offset), walk(offset), `offset ${offset}`);
  }
});
