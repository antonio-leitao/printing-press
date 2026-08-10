<script lang="ts">
  import { fade, fly } from 'svelte/transition';
  import { dismiss, toasts } from '$lib/messages.svelte';
</script>

<!-- Over everything, centred on the bottom edge, and never in the layout: the
     window is a fixed shell, so a message that took space would move the
     document under it. -->
<div class="toasts" aria-live="polite">
  {#each toasts as toast (toast.id)}
    <button
      class="toast {toast.tone}"
      onclick={() => dismiss(toast.id)}
      title="Dismiss"
      in:fly={{ y: 10, duration: 160 }}
      out:fade={{ duration: 120 }}
    >
      {toast.text}
    </button>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    bottom: 2.25rem;
    left: 50%;
    transform: translateX(-50%);
    z-index: 40;
    display: grid;
    justify-items: center;
    gap: 0.4rem;
    width: max-content;
    max-width: min(34rem, calc(100vw - 2rem));
    /* The stack is not a wall: clicks land on the document behind it, and only
       a message itself takes one. */
    pointer-events: none;
  }

  /* Tinted rather than filled: a message has to be seen at a glance without
     becoming the loudest thing on a page of near-whites. Each tone carries the
     colour it already means elsewhere — the accent for what went well, amber
     for what Press could not do, brick for what failed. */
  .toast {
    padding: 0.45rem 0.75rem;
    border: var(--bw) solid var(--accent-line);
    border-radius: var(--radius);
    background: var(--accent-tint);
    color: var(--accent-strong);
    box-shadow: var(--shadow-lg);
    font-size: var(--fs-menu);
    line-height: 1.35;
    text-align: center;
    /* A path has no spaces to break at, and one long one should not stretch
       the toast past the window. */
    overflow-wrap: anywhere;
    cursor: pointer;
    pointer-events: auto;
  }

  .toast.warning {
    border-color: var(--warning-line);
    background: var(--warning-tint);
    color: var(--warning-strong);
  }

  .toast.error {
    border-color: var(--danger-line);
    background: var(--danger-tint);
    color: var(--danger-strong);
  }
</style>
