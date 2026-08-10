import type { Action } from 'svelte/action';

/**
 * Makes a `<dialog>` a real modal one.
 *
 * `showModal()` rather than the `open` attribute. Only a modal dialog is put in
 * the top layer, keeps the keyboard inside itself, dims what is behind it and
 * treats Escape as cancel. An `open` dialog is none of that — it is a box drawn
 * over the page that you can tab straight out of and cannot dismiss.
 *
 * The dismissal is handed back rather than acted on here, because whether the
 * dialog exists at all is the page's state to decide. Closing the element behind
 * that state's back leaves the two disagreeing: gone from the screen, still open
 * as far as the application is concerned.
 */
export const modal: Action<HTMLDialogElement, () => void> = (element, dismiss) => {
  let onDismiss = dismiss;

  const cancel = (event: Event) => {
    event.preventDefault();
    onDismiss?.();
  };

  /**
   * Escape, read from the key rather than left to the `cancel` event.
   *
   * A user agent is supposed to raise `cancel` on a modal dialog when Escape is
   * pressed, and not every one of them does — the key arrives, nothing else
   * happens, and the dialog cannot be dismissed from the keyboard at all.
   * Reading the key is the same behaviour, owed to nobody. `cancel` is still
   * listened for, so a dialog dismissed the other way behaves identically;
   * asking for the same dismissal twice costs nothing.
   */
  const keydown = (event: KeyboardEvent) => {
    if (event.key !== 'Escape') return;
    event.preventDefault();
    onDismiss?.();
  };

  element.addEventListener('cancel', cancel);
  element.addEventListener('keydown', keydown);
  // Guarded: `showModal` on a dialog that is already open throws.
  if (!element.open) element.showModal();

  return {
    update(next: () => void) {
      onDismiss = next;
    },
    destroy() {
      element.removeEventListener('cancel', cancel);
      element.removeEventListener('keydown', keydown);
      if (element.open) element.close();
    }
  };
};
