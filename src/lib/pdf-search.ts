/**
 * Finding the way around a document's search hits, without touching the DOM so
 * it can be tested.
 *
 * Hits arrive in reading order: page by page, and down each page in the order
 * MuPDF read it. Everything here leans on that order rather than sorting again.
 */

import type { SearchHit } from './types';

/** Shared by every page without hits, so a search that clears changes nothing. */
const NONE: SearchHit[] = [];

/**
 * Each page's hits, keyed by its 0-based index. A page with none is not in the
 * map: `hitsOn` answers for it.
 */
export function groupByPage(hits: SearchHit[]): Map<number, SearchHit[]> {
  const pages = new Map<number, SearchHit[]>();
  for (const hit of hits) {
    const page = pages.get(hit.page);
    if (page) page.push(hit);
    else pages.set(hit.page, [hit]);
  }
  return pages;
}

export function hitsOn(pages: Map<number, SearchHit[]>, page: number): SearchHit[] {
  return pages.get(page) ?? NONE;
}

/**
 * The first hit at or after a place in the document, wrapping to the first
 * hit of all when there is nothing further on. That is where a search lands in
 * zathura and in vim: forward from where the reader is, never back to the top
 * of a document they are halfway through.
 *
 * `page` is 0-based and `top` is in PDF points from that page's top. -1 when
 * there are no hits at all.
 */
export function firstFrom(hits: SearchHit[], page: number, top: number): number {
  if (hits.length === 0) return -1;
  const found = hits.findIndex((hit) => hit.page > page || (hit.page === page && hit.y >= top));
  return found === -1 ? 0 : found;
}

/**
 * `count` hits on from `current`, in either direction, wrapping at both ends —
 * `n` past the last hit is the first one again, as it is in vim.
 */
export function stepFrom(current: number, length: number, sign: 1 | -1, count = 1): number {
  if (length === 0) return -1;
  const from = current < 0 ? (sign === 1 ? -1 : 0) : current;
  const moved = (from + sign * count) % length;
  return moved < 0 ? moved + length : moved;
}

/** What the prompt says about a search that has run. */
export function describe(current: number, length: number): string {
  if (length === 0) return 'No matches';
  return `${current + 1} of ${length}`;
}
