import assert from 'node:assert/strict';
import test from 'node:test';
import { modal } from '../src/lib/modal.ts';

/** Just the part of a <dialog> the action touches. */
function fakeDialog({ open = false } = {}) {
  return {
    open,
    calls: [],
    listeners: new Map(),
    showModal() {
      this.open = true;
      this.calls.push('showModal');
    },
    close() {
      this.open = false;
      this.calls.push('close');
    },
    addEventListener(type, handler) {
      this.listeners.set(type, handler);
    },
    removeEventListener(type) {
      this.listeners.delete(type);
    },
    fire(type, event = {}) {
      let prevented = false;
      this.listeners.get(type)?.({ ...event, preventDefault: () => (prevented = true) });
      return prevented;
    }
  };
}

test('opens the dialog modally', () => {
  const element = fakeDialog();
  modal(element, () => {});
  assert.deepEqual(element.calls, ['showModal']);
  assert.ok(element.open);
});

test('does not reopen a dialog that is already open', () => {
  // showModal throws on a dialog that is already open.
  const element = fakeDialog({ open: true });
  modal(element, () => {});
  assert.deepEqual(element.calls, []);
});

test('escape asks the page to take the dialog down rather than closing it', () => {
  const element = fakeDialog();
  let dismissed = 0;
  modal(element, () => (dismissed += 1));

  const prevented = element.fire('cancel');
  assert.ok(prevented, 'the browser must not close it behind the page state');
  assert.equal(dismissed, 1);
  // Still open: whatever renders it decides that, and it has just been asked to.
  assert.ok(element.open);
  assert.ok(!element.calls.includes('close'));
});

/// Not every user agent raises `cancel` on Escape, and one that does not left
/// the dialog undismissable from the keyboard. The key is read directly.
test('escape is read from the key rather than left to the user agent', () => {
  const element = fakeDialog();
  let dismissed = 0;
  modal(element, () => (dismissed += 1));

  const prevented = element.fire('keydown', { key: 'Escape' });
  assert.ok(prevented);
  assert.equal(dismissed, 1);
});

test('other keys are left alone', () => {
  const element = fakeDialog();
  let dismissed = 0;
  modal(element, () => (dismissed += 1));

  for (const key of ['Enter', 'Tab', 'a', 'ArrowDown']) {
    assert.ok(!element.fire('keydown', { key }), key);
  }
  assert.equal(dismissed, 0);
});

test('a replaced handler is the one that runs', () => {
  const element = fakeDialog();
  const seen = [];
  const action = modal(element, () => seen.push('first'));
  action.update(() => seen.push('second'));

  element.fire('cancel');
  element.fire('keydown', { key: 'Escape' });
  assert.deepEqual(seen, ['second', 'second']);
});

test('teardown closes the dialog and stops listening', () => {
  const element = fakeDialog();
  let dismissed = 0;
  const action = modal(element, () => (dismissed += 1));

  action.destroy();
  assert.ok(!element.open);
  assert.deepEqual(element.calls, ['showModal', 'close']);
  assert.equal(element.listeners.size, 0);
  // Nothing left to fire, and nothing left to be told about it.
  element.fire('cancel');
  assert.equal(dismissed, 0);
});
