/**
 * The display's pixel ratio, as something a page can depend on.
 *
 * Every page is drawn for the screen it is on: at the ratio of a retina panel,
 * four times the pixels of an ordinary one. `devicePixelRatio` changes when the
 * window is dragged from one to the other, but reading it is not something
 * Svelte can know has changed, so pages drawn for one screen stayed drawn for
 * it — blurred on the retina, or carrying four times the memory they need on
 * the other.
 *
 * The platform's only notice is a media query for the current ratio that stops
 * matching. It is asked again for the new ratio each time, because a query only
 * ever describes the one value it was written for.
 */

/// Anything that cannot say is treated as an ordinary screen.
function current(): number {
  if (typeof window === 'undefined') return 1;
  const ratio = window.devicePixelRatio;
  return Number.isFinite(ratio) && ratio > 0 ? ratio : 1;
}

let ratio = $state(current());

/// Deferred until the browser is there to be asked, like the theme.
export function startDisplay() {
  ratio = current();
  if (typeof window === 'undefined' || !window.matchMedia) return;
  const watch = () => {
    window
      .matchMedia(`(resolution: ${ratio}dppx)`)
      .addEventListener(
        'change',
        () => {
          ratio = current();
          watch();
        },
        { once: true }
      );
  };
  watch();
}

export const display = {
  /** Device pixels per CSS pixel, kept current as the window moves. */
  get pixelRatio() {
    return ratio;
  }
};
