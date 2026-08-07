import { errorMessage } from '$lib/api';

/**
 * The app's messages. Anything that used to be written into a bar at the edge
 * of the window goes through here instead: one floating stack, at the bottom,
 * over whatever is on screen, gone on its own.
 *
 * Deliberately small. There is no queue, no priority and no history — a
 * message is either worth a glance now or it is not a message.
 */
export type Tone = 'info' | 'warning' | 'error';

export type Toast = {
  id: number;
  text: string;
  tone: Tone;
};

/** An error is read, not glanced at, so it is given longer. */
const LIFETIME: Record<Tone, number> = { info: 4500, warning: 7000, error: 9000 };

/** Older messages are dropped rather than stacked past this. */
const MOST = 3;

export const toasts = $state<Toast[]>([]);

let nextId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();

/// Shows a message. Repeating one that is already up refreshes it rather than
/// stacking a second copy: a watcher that fails twice is still one problem.
export function notify(text: string, tone: Tone = 'info') {
  const message = text.trim();
  if (!message) return;

  const showing = toasts.find((toast) => toast.text === message && toast.tone === tone);
  const id = showing?.id ?? nextId++;
  if (showing) {
    clearTimeout(timers.get(id));
  } else {
    toasts.push({ id, text: message, tone });
    while (toasts.length > MOST) dismiss(toasts[0].id);
  }
  timers.set(
    id,
    setTimeout(() => dismiss(id), LIFETIME[tone])
  );
}

/// Shows whatever a rejected command carried, which is nearly always a string
/// from the backend.
export function fail(reason: unknown) {
  notify(errorMessage(reason), 'error');
}

export function dismiss(id: number) {
  clearTimeout(timers.get(id));
  timers.delete(id);
  const index = toasts.findIndex((toast) => toast.id === id);
  if (index !== -1) toasts.splice(index, 1);
}
