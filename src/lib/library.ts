/**
 * Arranging the library: grouping documents by the folders they live in, and
 * narrowing them down to the ones a few typed words describe. Kept away from
 * the DOM so it can be tested.
 *
 * The groups come from the paths and nowhere else. Every document's folder is
 * placed in one tree under the folder all of them share, and a folder whose
 * only content is one other folder is folded into it — `Repls/presentation` is
 * one step, not two, because nothing could ever be grouped at `Repls` alone.
 * The grouping level is then how many of those steps a group is named by. One
 * gives the broadest groups; each step after that splits them further.
 */

/** What grouping needs to know about a document. */
export type Shelved = {
  /** The folder the document is in, absolute. */
  directory: string;
  lastOpenedAt: number;
};

export type FolderGroup<T> = {
  /**
   * The folder the group stands for, absolute. Stable for as long as the
   * folder is, which is what lets a collapsed group stay collapsed.
   */
  key: string;
  /** The steps above the one the group is named after, outermost first. */
  parents: string[];
  /** A step can be several folders long: `PLS_Pulsar/paper`. */
  name: string;
  items: T[];
};

export type Grouping<T> = {
  groups: FolderGroup<T>[];
  /**
   * Documents that would be a group of one at this level. A group of one is a
   * document, so these are shown as documents.
   */
  loose: T[];
};

type Folder = { children: Map<string, Folder>; held: number };

function segments(directory: string): string[] {
  return directory.split('/').filter(Boolean);
}

/**
 * Each directory's steps below the folder they all share, with runs of
 * single-child folders folded into one step.
 */
export function folderSteps(directories: string[]): { root: string; steps: string[][] } {
  if (directories.length === 0) return { root: '/', steps: [] };
  const split = directories.map(segments);
  let shared = 0;
  while (split.every((parts) => parts.length > shared && parts[shared] === split[0][shared])) {
    shared += 1;
  }

  const tree: Folder = { children: new Map(), held: 0 };
  for (const parts of split) {
    let folder = tree;
    for (const part of parts.slice(shared)) {
      let child = folder.children.get(part);
      if (!child) {
        child = { children: new Map(), held: 0 };
        folder.children.set(part, child);
      }
      folder = child;
    }
    folder.held += 1;
  }

  const steps = split.map((parts) => {
    const out: string[] = [];
    let folder = tree;
    let index = shared;
    while (index < parts.length) {
      let step = parts[index];
      folder = folder.children.get(step)!;
      index += 1;
      // Folded while the folder holds nothing of its own and leads one way.
      while (index < parts.length && folder.held === 0 && folder.children.size === 1) {
        step += `/${parts[index]}`;
        folder = folder.children.get(parts[index])!;
        index += 1;
      }
      out.push(step);
    }
    return out;
  });
  return { root: `/${split[0].slice(0, shared).join('/')}`, steps };
}

/** How many levels of grouping these documents have to offer. Zero means none. */
export function groupingDepth(items: Shelved[]): number {
  const { steps } = folderSteps(items.map((item) => item.directory));
  return steps.reduce((deepest, chain) => Math.max(deepest, chain.length), 0);
}

/**
 * The documents grouped `level` steps deep. Level zero, or more than the
 * library is deep, is clamped rather than refused.
 *
 * Groups are ordered by the most recently opened document in each, and the
 * documents in a group by when each was last opened — the same order the
 * library already keeps, applied inside every group.
 */
export function groupByFolder<T extends Shelved>(items: T[], level: number): Grouping<T> {
  const recent = (a: T, b: T) => b.lastOpenedAt - a.lastOpenedAt;
  const ordered = [...items].sort(recent);
  if (level <= 0) return { groups: [], loose: ordered };

  const { root, steps } = folderSteps(ordered.map((item) => item.directory));
  const byKey = new Map<string, { chain: string[]; items: T[] }>();
  const loose: T[] = [];
  ordered.forEach((item, index) => {
    const chain = steps[index].slice(0, level);
    // At the shared folder itself: above every group, so in none of them.
    if (chain.length === 0) {
      loose.push(item);
      return;
    }
    const key = `${root === '/' ? '' : root}/${chain.join('/')}`;
    const group = byKey.get(key);
    if (group) group.items.push(item);
    else byKey.set(key, { chain, items: [item] });
  });

  const groups: FolderGroup<T>[] = [];
  for (const [key, { chain, items: members }] of byKey) {
    if (members.length < 2) {
      loose.push(...members);
      continue;
    }
    groups.push({ key, parents: chain.slice(0, -1), name: chain[chain.length - 1], items: members });
  }
  loose.sort(recent);
  return { groups, loose };
}

/**
 * A grouping with only the documents `keep` accepts, and without the groups
 * left empty. Narrowed after grouping rather than grouped after narrowing: the
 * groups are the library's folders, and a filter should show where its matches
 * live — not regroup the handful it found around whatever folder they share.
 */
export function narrowGrouping<T>(grouping: Grouping<T>, keep: (item: T) => boolean): Grouping<T> {
  return {
    groups: grouping.groups
      .map((group) => ({ ...group, items: group.items.filter(keep) }))
      .filter((group) => group.items.length > 0),
    loose: grouping.loose.filter(keep)
  };
}

/** What a typed filter looks at. */
export type Findable = { name: string; documentPath: string };

/**
 * Whether a document is one the filter describes. Every word has to appear,
 * in the name or anywhere in the path, ignoring case — so `thesis intro`
 * finds `thesis/chapters/intro.tex` without either word having to be first.
 */
export function matchesFilter(item: Findable, filter: string): boolean {
  const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const haystack = `${item.name}\n${item.documentPath}`.toLowerCase();
  return words.every((word) => haystack.includes(word));
}
