import assert from 'node:assert/strict';
import test from 'node:test';
import { initialKeyState, resolveKey } from '../src/lib/pdf-keys.ts';

function press(key, modifiers = {}) {
  return {
    key,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    altKey: false,
    ...modifiers
  };
}

/** Feeds a sequence and returns the last resolution plus the final state. */
function type(keys, state = initialKeyState()) {
  let resolution = { kind: 'ignored' };
  for (const key of keys) {
    const event = typeof key === 'string' ? press(key) : press(key.key, key);
    ({ resolution, state } = resolveKey(event, state));
  }
  return { resolution, state };
}

test('j and k scroll by a step', () => {
  const down = type(['j']).resolution;
  assert.deepEqual(down.action, {
    kind: 'scroll',
    axis: 'y',
    amount: 'step',
    sign: 1,
    count: 1
  });
  assert.equal(type(['k']).resolution.action.sign, -1);
});

test('h and l scroll sideways for a zoomed page', () => {
  assert.equal(type(['l']).resolution.action.axis, 'x');
  assert.equal(type(['h']).resolution.action.sign, -1);
});

test('half and full page movement, plain and with control', () => {
  assert.equal(type(['d']).resolution.action.amount, 'half');
  assert.equal(type(['u']).resolution.action.sign, -1);
  assert.equal(type(['f']).resolution.action.amount, 'page');
  assert.equal(type([{ key: 'd', ctrlKey: true }]).resolution.action.amount, 'half');
  assert.equal(type([{ key: 'b', ctrlKey: true }]).resolution.action.sign, -1);
});

test('space pages forward and shift-space back', () => {
  assert.equal(type([' ']).resolution.action.sign, 1);
  assert.equal(type([{ key: ' ', shiftKey: true }]).resolution.action.sign, -1);
});

test('gg waits for its second g', () => {
  const first = resolveKey(press('g'), initialKeyState());
  assert.equal(first.resolution.kind, 'pending');
  assert.equal(first.state.awaitingG, true);

  const second = resolveKey(press('g'), first.state);
  assert.deepEqual(second.resolution.action, { kind: 'goto', target: 'first' });
  assert.equal(second.state.awaitingG, false);
});

test('a key that is not g abandons the pending g and still acts', () => {
  const pending = resolveKey(press('g'), initialKeyState());
  const next = resolveKey(press('j'), pending.state);
  assert.equal(next.resolution.action.kind, 'scroll');
  assert.equal(next.state.awaitingG, false);
});

test('G goes to the end, and a count sends it to a page', () => {
  assert.deepEqual(type(['G']).resolution.action, { kind: 'goto', target: 'last' });
  assert.deepEqual(type(['1', '2', 'G']).resolution.action, {
    kind: 'goto',
    target: 'page',
    page: 12
  });
  // The vim spelling with gg works too.
  assert.deepEqual(type(['7', 'g', 'g']).resolution.action, {
    kind: 'goto',
    target: 'page',
    page: 7
  });
});

test('counts multiply movement and then reset', () => {
  const { resolution, state } = type(['5', 'j']);
  assert.equal(resolution.action.count, 5);
  assert.equal(state.count, '');
  // The next bare j is a single step again.
  assert.equal(type(['j'], state).resolution.action.count, 1);
});

test('zero is a zoom reset until a count is under way', () => {
  assert.deepEqual(type(['0']).resolution.action, { kind: 'fit', mode: 'actual' });
  // Inside a count it is a digit, so 10j means ten steps.
  assert.equal(type(['1', '0', 'j']).resolution.action.count, 10);
});

test('J and K move whole pages', () => {
  assert.deepEqual(type(['J']).resolution.action, { kind: 'page', sign: 1, count: 1 });
  assert.deepEqual(type(['3', 'K']).resolution.action, { kind: 'page', sign: -1, count: 3 });
});

test('zoom and fit keys follow zathura', () => {
  assert.deepEqual(type(['+']).resolution.action, { kind: 'zoom', step: 1 });
  assert.deepEqual(type(['-']).resolution.action, { kind: 'zoom', step: -1 });
  assert.deepEqual(type(['a']).resolution.action, { kind: 'fit', mode: 'page' });
  assert.deepEqual(type(['s']).resolution.action, { kind: 'fit', mode: 'width' });
});

test('command and alt chords are left to the platform', () => {
  assert.equal(type([{ key: 'j', metaKey: true }]).resolution.kind, 'ignored');
  assert.equal(type([{ key: 'j', altKey: true }]).resolution.kind, 'ignored');
});

test('escape clears a half-typed command', () => {
  const counted = type(['1', '2']);
  assert.equal(counted.state.count, '12');
  const cleared = resolveKey(press('Escape'), counted.state);
  assert.equal(cleared.state.count, '');
  assert.equal(cleared.state.awaitingG, false);
});

test('unknown keys clear pending input rather than leaving it stuck', () => {
  const counted = type(['4']);
  const { resolution, state } = resolveKey(press('q'), counted.state);
  assert.equal(resolution.kind, 'ignored');
  assert.equal(state.count, '');
});

test('arrow keys mirror hjkl', () => {
  assert.equal(type(['ArrowDown']).resolution.action.sign, 1);
  assert.equal(type(['ArrowUp']).resolution.action.sign, -1);
  assert.equal(type(['ArrowLeft']).resolution.action.axis, 'x');
  assert.equal(type(['PageDown']).resolution.action.amount, 'page');
  assert.deepEqual(type(['Home']).resolution.action, { kind: 'goto', target: 'first' });
});
