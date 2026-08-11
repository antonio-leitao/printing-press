/**
 * Which of the two themes Press is wearing.
 *
 * One switch, and everything follows it: the interface, and which way round a
 * page is drawn — a dark Press shows dark pages, in the reader and on the
 * shelf. Two ways to throw it, the button in the library and `⌃r` in the
 * viewer, and they are the same switch rather than two that have to agree.
 *
 * This does mean one document cannot be read light while another is read dark.
 * That was possible when reading in the dark was a property of the document,
 * and it cost more than it was worth: a page turned dark inside a Press that
 * had stayed light, which is not a document read in the dark so much as a
 * document at odds with the room.
 *
 * There is no media query anywhere. The preference is seeded from the system's
 * the first time Press runs and is a stored choice from then on, which is what
 * lets an explicit light beat a system dark; doing that in CSS would mean
 * writing the dark theme out twice.
 */
export type Theme = 'light' | 'dark';

const STORAGE_KEY = 'press:theme';

/// What the system is set to, for the first run only. Anything that cannot
/// answer — a webview without `matchMedia` — is treated as light.
function preferred(): Theme {
  if (typeof window === 'undefined' || !window.matchMedia) return 'light';
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

function stored(): Theme | null {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return saved === 'light' || saved === 'dark' ? saved : null;
  } catch {
    // Storage a webview will not give us is not worth failing to start over.
    return null;
  }
}

let chosen = $state<Theme>('light');

/// Deferred until the browser is there to be asked: this module is imported by
/// a layout that also renders on the server.
export function startTheme() {
  chosen = stored() ?? preferred();
}

export const theme = {
  /** The preference. Light or dark, and it is remembered. */
  get chosen() {
    return chosen;
  },
  set chosen(next: Theme) {
    chosen = next;
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // A choice that cannot be written down still applies to this session.
    }
  },

  /**
   * The same question as `chosen`, asked the way most callers want it: is a
   * page drawn inverted, is a thumbnail, is the room dark.
   */
  get isDark() {
    return chosen === 'dark';
  },

  toggle() {
    theme.chosen = chosen === 'dark' ? 'light' : 'dark';
  }
};
