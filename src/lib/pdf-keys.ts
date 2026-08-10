/**
 * Zathura's keymap, resolved without touching the DOM so it can be tested.
 *
 * Counts work the way they do in vim: digits accumulate into a prefix, and the
 * next command consumes them. `0` is only a count digit once a count has been
 * started, otherwise it is the zoom reset.
 */

export type ViewerAction =
  | { kind: 'scroll'; axis: 'x' | 'y'; amount: 'step' | 'half' | 'page'; sign: 1 | -1; count: number }
  | { kind: 'goto'; target: 'first' | 'last' | 'page'; page?: number }
  | { kind: 'page'; sign: 1 | -1; count: number }
  | { kind: 'zoom'; step: number }
  | { kind: 'fit'; mode: 'actual' | 'page' | 'width' }
  /** Back or forward through the places a jump was made from. */
  | { kind: 'jump'; sign: 1 | -1 }
  /** Draw the page for a dark room, or stop. */
  | { kind: 'invert' };

export type KeyResolution =
  | { kind: 'action'; action: ViewerAction }
  | { kind: 'pending' }
  | { kind: 'ignored' };

export type KeyState = {
  /** Digits typed so far, as text so a leading zero cannot be lost. */
  count: string;
  /** A `g` is waiting for its second `g`. */
  awaitingG: boolean;
};

export function initialKeyState(): KeyState {
  return { count: '', awaitingG: false };
}

export type KeyEventLike = {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
};

/**
 * Resolves one keystroke. Returns the action to run and the next state; the
 * caller decides what a scroll step is in pixels.
 */
export function resolveKey(
  event: KeyEventLike,
  state: KeyState
): { resolution: KeyResolution; state: KeyState } {
  const clear = initialKeyState();
  // Command and Alt chords belong to the platform, not to the document.
  if (event.metaKey || event.altKey) {
    return { resolution: { kind: 'ignored' }, state };
  }

  const count = state.count === '' ? 1 : Math.min(Number(state.count), 100000);
  const done = (action: ViewerAction): { resolution: KeyResolution; state: KeyState } => ({
    resolution: { kind: 'action', action },
    state: clear
  });

  if (event.ctrlKey) {
    switch (event.key.toLowerCase()) {
      case 'd':
        return done({ kind: 'scroll', axis: 'y', amount: 'half', sign: 1, count });
      case 'u':
        return done({ kind: 'scroll', axis: 'y', amount: 'half', sign: -1, count });
      case 'f':
        return done({ kind: 'scroll', axis: 'y', amount: 'page', sign: 1, count });
      case 'b':
        return done({ kind: 'scroll', axis: 'y', amount: 'page', sign: -1, count });
      // vim's jumplist, which is also zathura's: back to where the jump was
      // made from, and forward again.
      case 'o':
        return done({ kind: 'jump', sign: -1 });
      case 'i':
        return done({ kind: 'jump', sign: 1 });
      // zathura's `recolor`, on the key it uses for it.
      case 'r':
        return done({ kind: 'invert' });
      default:
        return { resolution: { kind: 'ignored' }, state };
    }
  }

  // A count in progress swallows digits, including zero.
  if (/^[0-9]$/.test(event.key) && !(event.key === '0' && state.count === '')) {
    return {
      resolution: { kind: 'pending' },
      state: { count: state.count + event.key, awaitingG: false }
    };
  }

  if (state.awaitingG) {
    if (event.key === 'g') {
      return state.count === ''
        ? done({ kind: 'goto', target: 'first' })
        : done({ kind: 'goto', target: 'page', page: count });
    }
    // Anything else abandons the pending `g`, then is handled on its own.
    return resolveKey(event, { count: state.count, awaitingG: false });
  }

  switch (event.key) {
    case 'j':
      return done({ kind: 'scroll', axis: 'y', amount: 'step', sign: 1, count });
    case 'k':
      return done({ kind: 'scroll', axis: 'y', amount: 'step', sign: -1, count });
    case 'h':
      return done({ kind: 'scroll', axis: 'x', amount: 'step', sign: -1, count });
    case 'l':
      return done({ kind: 'scroll', axis: 'x', amount: 'step', sign: 1, count });
    case 'ArrowDown':
      return done({ kind: 'scroll', axis: 'y', amount: 'step', sign: 1, count });
    case 'ArrowUp':
      return done({ kind: 'scroll', axis: 'y', amount: 'step', sign: -1, count });
    case 'ArrowRight':
      return done({ kind: 'scroll', axis: 'x', amount: 'step', sign: 1, count });
    case 'ArrowLeft':
      return done({ kind: 'scroll', axis: 'x', amount: 'step', sign: -1, count });
    case 'd':
      return done({ kind: 'scroll', axis: 'y', amount: 'half', sign: 1, count });
    case 'u':
      return done({ kind: 'scroll', axis: 'y', amount: 'half', sign: -1, count });
    case 'f':
    case 'PageDown':
      return done({ kind: 'scroll', axis: 'y', amount: 'page', sign: 1, count });
    case 'b':
    case 'PageUp':
      return done({ kind: 'scroll', axis: 'y', amount: 'page', sign: -1, count });
    case ' ':
      return done({
        kind: 'scroll',
        axis: 'y',
        amount: 'page',
        sign: event.shiftKey ? -1 : 1,
        count
      });
    case 'J':
      return done({ kind: 'page', sign: 1, count });
    case 'K':
      return done({ kind: 'page', sign: -1, count });
    case 'g':
      return { resolution: { kind: 'pending' }, state: { count: state.count, awaitingG: true } };
    case 'G':
      return state.count === ''
        ? done({ kind: 'goto', target: 'last' })
        : done({ kind: 'goto', target: 'page', page: count });
    case 'Home':
      return done({ kind: 'goto', target: 'first' });
    case 'End':
      return done({ kind: 'goto', target: 'last' });
    case '+':
    case '=':
      return done({ kind: 'zoom', step: count });
    case '-':
    case '_':
      return done({ kind: 'zoom', step: -count });
    case '0':
      return done({ kind: 'fit', mode: 'actual' });
    case 'a':
      return done({ kind: 'fit', mode: 'page' });
    case 's':
      return done({ kind: 'fit', mode: 'width' });
    case 'Escape':
      return { resolution: { kind: 'ignored' }, state: clear };
    default:
      return { resolution: { kind: 'ignored' }, state: clear };
  }
}
