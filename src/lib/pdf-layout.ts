/**
 * Finding a page in a column of them.
 *
 * The viewer asks which page it is looking at on every scroll frame. Answering
 * it by walking the column measures every page above the one being read, and
 * measuring a page means reading its position out of the layout — three hundred
 * of them, sixty times a second, at the end of a thesis.
 */

/**
 * The last index whose position is at or before `offset`, or -1 when there is
 * nothing to search.
 *
 * `positionOf` is asked for about `log2(count)` positions rather than all of
 * them, which is the whole point of doing it this way. Positions must not
 * decrease as the index grows — which a column of pages guarantees, whatever
 * their individual sizes.
 */
export function indexAt(
  count: number,
  offset: number,
  positionOf: (index: number) => number
): number {
  if (count <= 0) return -1;
  let low = 0;
  let high = count - 1;
  // The first page, for an offset above the top of the document: overscrolling
  // upwards is still looking at page one.
  let found = 0;
  while (low <= high) {
    const middle = (low + high) >> 1;
    if (offset < positionOf(middle)) {
      high = middle - 1;
    } else {
      found = middle;
      low = middle + 1;
    }
  }
  return found;
}
