import assert from 'node:assert/strict';
import test from 'node:test';
import {
  folderSteps,
  groupByFolder,
  groupingDepth,
  matchesFilter,
  narrowGrouping
} from '../src/lib/library.ts';

/** A library laid out the way a real one grows: unevenly. */
const HOME = '/home/reader/Documents';
let clock = 100;
function doc(name, directory) {
  clock -= 1;
  return { name, directory: `${HOME}/${directory}`, documentPath: `${HOME}/${directory}/${name}`, lastOpenedAt: clock };
}
const library = [
  doc('Test2.md', 'Personal/notes'),
  doc('main.tex', 'Repls/presentation'),
  doc('main.tex', 'Projects/XAI/paper'),
  doc('main.tex', 'Personal/writing/office'),
  doc('pitch.tex', 'Projects/Pulsar/paper'),
  doc('main.tex', 'Projects/Pulsar/paper'),
  doc('reviewers.tex', 'Research/climate'),
  doc('main.tex', 'Personal/writing/dreams'),
  doc('Test3.md', 'Personal/notes'),
  doc('intro.md', 'Personal/writing/dreams'),
  doc('craft.md', 'Personal/writing'),
  doc('intro.md', 'Personal/writing/apology'),
  doc('main.tex', 'Personal/writing/foundations')
];

const names = (grouping) => grouping.groups.map((group) => [...group.parents, group.name].join(' / '));

test('a folder that only leads to one other folder is one step', () => {
  const { root, steps } = folderSteps(library.map((item) => item.directory));
  assert.equal(root, HOME);
  assert.deepEqual(steps[1], ['Repls/presentation']);
  assert.deepEqual(steps[2], ['Projects', 'XAI/paper']);
  assert.deepEqual(steps[3], ['Personal', 'writing', 'office']);
  // A folder that holds a document of its own is never folded away.
  assert.deepEqual(steps[10], ['Personal', 'writing']);
});

test('each level splits the groups further', () => {
  assert.equal(groupingDepth(library), 3);
  assert.deepEqual(names(groupByFolder(library, 1)), ['Personal', 'Projects']);
  assert.deepEqual(names(groupByFolder(library, 2)), [
    'Personal / notes',
    'Personal / writing',
    'Projects / Pulsar/paper'
  ]);
  assert.deepEqual(names(groupByFolder(library, 3)), [
    'Personal / notes',
    'Projects / Pulsar/paper',
    'Personal / writing / dreams'
  ]);
});

test('a group of one is a document, not a group', () => {
  const { loose } = groupByFolder(library, 2);
  assert.deepEqual(
    loose.map((item) => item.directory.slice(HOME.length + 1)),
    ['Repls/presentation', 'Projects/XAI/paper', 'Research/climate']
  );
  // Nothing goes missing between the groups and the loose documents.
  for (let level = 0; level <= 4; level += 1) {
    const grouping = groupByFolder(library, level);
    const placed = grouping.loose.length + grouping.groups.reduce((sum, group) => sum + group.items.length, 0);
    assert.equal(placed, library.length, `level ${level}`);
  }
});

test('groups and the documents in them are ordered by when they were last opened', () => {
  const shuffled = [...library].reverse();
  const { groups } = groupByFolder(shuffled, 2);
  assert.equal(groups[0].name, 'notes', 'holds the most recently opened document');
  const opened = groups[1].items.map((item) => item.lastOpenedAt);
  assert.deepEqual(opened, [...opened].sort((a, b) => b - a));
});

test('a group is keyed by its folder, so it stays itself as the library changes', () => {
  const before = groupByFolder(library, 2).groups.find((group) => group.name === 'writing');
  assert.equal(before.key, `${HOME}/Personal/writing`);
  // A document outside Documents moves the shared folder up a level, which
  // moves every group one step deeper — under the same key.
  const outside = { name: 'loose.tex', directory: '/home/reader/Desktop', documentPath: '/home/reader/Desktop/loose.tex', lastOpenedAt: 0 };
  const after = groupByFolder([...library, outside], 3).groups.find((group) => group.name === 'writing');
  assert.equal(after.key, before.key);
  assert.deepEqual(after.parents, ['Documents', 'Personal']);
});

test('one folder, or none, has nothing to group by', () => {
  assert.equal(groupingDepth([]), 0);
  assert.equal(groupingDepth([doc('a.tex', 'paper'), doc('b.tex', 'paper')]), 0);
  const { groups, loose } = groupByFolder([doc('a.tex', 'paper'), doc('b.tex', 'paper')], 2);
  assert.equal(groups.length, 0);
  assert.equal(loose.length, 2);
});

test('a filter keeps the library’s groups and drops the ones it empties', () => {
  const narrowed = narrowGrouping(groupByFolder(library, 2), (item) => item.directory.includes('Pulsar'));
  assert.deepEqual(names(narrowed), ['Projects / Pulsar/paper'], 'the matches stay in their own group');
  assert.equal(narrowed.groups[0].items.length, 2);
  assert.deepEqual(narrowed.loose, []);
});

test('a filter matches every word, anywhere in the name or path, in any case', () => {
  const thesis = { name: 'My Thesis', documentPath: '/home/reader/thesis/chapters/intro.tex' };
  assert.ok(matchesFilter(thesis, ''));
  assert.ok(matchesFilter(thesis, '  '));
  assert.ok(matchesFilter(thesis, 'thesis'));
  assert.ok(matchesFilter(thesis, 'INTRO thesis'));
  assert.ok(matchesFilter(thesis, 'chapters/intro'));
  assert.ok(!matchesFilter(thesis, 'thesis outro'));
});
