/**
 * Which of the two themes Press is wearing, and who decides.
 *
 * One switch, and everything follows it: the interface, and which way round a
 * page is drawn — a dark Press shows dark pages, in the reader and on the
 * shelf. Two ways to throw it, Settings and `⌃r` in the viewer, and they are
 * the same switch rather than two that have to agree.
 *
 * This does mean one document cannot be read light while another is read dark.
 * That was possible when reading in the dark was a property of the document,
 * and it cost more than it was worth: a page turned dark inside a Press that
 * had stayed light, which is not a document read in the dark so much as a
 * document at odds with the room.
 *
 * Three preferences, two themes. `system` is not a third way for a page to look
 * — it is a way of declining to choose, and what it resolves to is whatever
 * macOS is set to at the moment it is asked. The webview inherits the
 * application's appearance, so `prefers-color-scheme` is the system's own
 * setting arriving by the shortest route available to us; the listener below is
 * what makes it live rather than read once.
 *
 * `preference` is what the reader said and `resolved` is what that means right
 * now, and keeping them apart is the whole of the design. Only `resolved` may
 * reach the stylesheet: `data-theme="system"` matches nothing and would put
 * Press in the light theme by accident.
 */
export type Theme = 'light' | 'dark';
export type ThemePreference = Theme | 'system';

const STORAGE_KEY = 'press:theme';

/** The three, in the order Settings offers them. */
export const THEME_PREFERENCES: readonly ThemePreference[] = ['light', 'dark', 'system'];

const QUERY = '(prefers-color-scheme: dark)';

/// What the system is set to. Anything that cannot answer — a webview without
/// `matchMedia` — is treated as light.
function systemTheme(): Theme {
  if (typeof window === 'undefined' || !window.matchMedia) return 'light';
  return window.matchMedia(QUERY).matches ? 'dark' : 'light';
}

function stored(): ThemePreference | null {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return saved === 'light' || saved === 'dark' || saved === 'system' ? saved : null;
  } catch {
    // Storage a webview will not give us is not worth failing to start over.
    return null;
  }
}

let preference = $state<ThemePreference>('system');
/// The system's answer, kept current rather than asked for on every read: a
/// getter that runs `matchMedia` is not something Svelte can know has changed.
let system = $state<Theme>('light');

/// Deferred until the browser is there to be asked: this module is imported by
/// a layout that also renders on the server.
export function startTheme() {
  system = systemTheme();
  // No stored preference means a first run, and a first run follows the system
  // rather than guessing. That is also what the old seeding did, except that it
  // froze the answer; this one keeps listening until told otherwise.
  preference = stored() ?? 'system';

  if (typeof window === 'undefined' || !window.matchMedia) return;
  // Listened to whatever the preference is. Following it only matters under
  // `system`, but keeping `system` current costs nothing and means switching
  // back to it never shows a stale answer for a frame.
  window.matchMedia(QUERY).addEventListener('change', (event) => {
    system = event.matches ? 'dark' : 'light';
  });
}

export const theme = {
  /** What the reader chose: light, dark, or leave it to the system. */
  get preference() {
    return preference;
  },
  set preference(next: ThemePreference) {
    preference = next;
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // A choice that cannot be written down still applies to this session.
    }
  },

  /**
   * Which theme is actually on. The only one the stylesheet may be told about,
   * and the one every caller asking "is it dark" wants.
   */
  get resolved(): Theme {
    return preference === 'system' ? system : preference;
  },

  /**
   * The same question as `resolved`, asked the way most callers want it: is a
   * page drawn inverted, is a thumbnail, is the room dark.
   */
  get isDark() {
    return theme.resolved === 'dark';
  },

  /**
   * `⌃r`. Throws the switch to the other theme, which means naming one: a
   * keystroke that cycled through `system` would leave the reader somewhere
   * they cannot see from the key they pressed. Choosing the system again is a
   * thing to do in Settings, deliberately.
   */
  toggle() {
    theme.preference = theme.resolved === 'dark' ? 'light' : 'dark';
  }
};
