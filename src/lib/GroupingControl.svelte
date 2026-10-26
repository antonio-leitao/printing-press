<script lang="ts">
  /* How many folder levels the library is grouped by. In the bar it is one
     quiet icon that draws the level, one indent per level; clicking it opens a
     small panel with the slider, a name for each stop and how many groups the
     level makes.

     A panel rather than the slider in the bar, because the level is set once
     and then left alone. The bar keeps the icon, which says how deep the
     grouping goes without a word, and the panel has room for the words.

     The track, the filled part and the ticks are drawn behind a native range
     input whose own track is transparent, so dragging and the arrow keys are
     the browser's and only the look is ours. */
  import { tick } from 'svelte';

  let {
    level = $bindable(0),
    limit,
    groups
  }: {
    level?: number;
    /** The deepest level the library has. At least one, or there is no control. */
    limit: number;
    /** How many groups the current level makes. */
    groups: number;
  } = $props();

  let open = $state(false);
  let root = $state<HTMLElement | null>(null);
  let trigger = $state<HTMLButtonElement | null>(null);
  let slider = $state<HTMLInputElement | null>(null);

  /* The track is 210px and the thumb 12px, so the thumb's centre travels
     198px: 6px in from either end, the stops evenly spaced between. */
  const WIDTH = 210;
  const THUMB = 12;
  const centre = (stop: number) => THUMB / 2 + (stop * (WIDTH - THUMB)) / limit;

  const shown = $derived(Math.min(level, limit));
  const stops = $derived(
    Array.from({ length: limit + 1 }, (_, stop) => ({
      at: centre(stop),
      name: stop === 0 ? 'Off' : String(stop)
    }))
  );
  const count = $derived(
    shown === 0 ? 'Not grouped' : `${groups} ${groups === 1 ? 'group' : 'groups'}`
  );
  const label = $derived(
    shown === 0
      ? 'Group by folder: off'
      : `Group by folder: ${shown} ${shown === 1 ? 'level' : 'levels'} · ${count}`
  );

  /* One indent per level, as far as three. A library deeper than that still
     gets every stop on the slider; the icon has said "deep" by then. */
  const GLYPHS = [
    'M4 6h.01M4 12h.01M4 18h.01M8 6h12M8 12h12M8 18h12',
    'M4 5h.01M8 5h12M8 12h.01M12 12h8M8 19h.01M12 19h8',
    'M4 5h.01M8 5h12M8 12h.01M12 12h8M12 19h.01M16 19h4',
    'M3 4h.01M6.5 4H21M7 9.5h.01M10.5 9.5H21M11 15h.01M14.5 15H21M15 20.5h.01M18.5 20.5H21'
  ];
  const glyph = $derived(GLYPHS[Math.min(shown, GLYPHS.length - 1)]);

  /// Opening puts the keyboard on the slider, so the arrow keys change the
  /// level straight away.
  async function toggle() {
    open = !open;
    if (!open) return;
    await tick();
    slider?.focus();
  }

  function keys(event: KeyboardEvent) {
    if (!open || event.key !== 'Escape') return;
    event.preventDefault();
    event.stopPropagation();
    open = false;
    trigger?.focus();
  }

  // A press anywhere outside closes the panel, the way a menu closes.
  $effect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (root && !root.contains(event.target as Node)) open = false;
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  });
</script>

<svelte:window onkeydown={keys} />

<div class="grouping" bind:this={root}>
  <button
    bind:this={trigger}
    class="level-icon"
    class:flat={shown === 0}
    class:open
    onclick={toggle}
    title={label}
    aria-label={label}
    aria-expanded={open}
    aria-controls="grouping-panel"
  >
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      aria-hidden="true"
    >
      <path d={glyph} />
    </svg>
  </button>
  {#if open}
    <div class="panel" id="grouping-panel" role="group" aria-label="Group by folder">
      <div class="heading">
        <span>Group by folder</span>
        <span class="count">{count}</span>
      </div>
      <span class="slider" style:width="{WIDTH}px">
        <span class="groove"></span>
        <span class="filled" style:width="{centre(shown)}px"></span>
        {#each stops as stop (stop.at)}
          <span class="tick" style:left="{stop.at - 0.5}px"></span>
        {/each}
        <input
          bind:this={slider}
          type="range"
          min="0"
          max={limit}
          step="1"
          value={shown}
          oninput={(event) => (level = Number(event.currentTarget.value))}
          aria-label="Group by folder"
          aria-valuetext={label}
        />
      </span>
      <span class="stops" style:width="{WIDTH}px" aria-hidden="true">
        {#each stops as stop (stop.at)}
          <span style:left="{stop.at}px">{stop.name}</span>
        {/each}
      </span>
    </div>
  {/if}
</div>

<style>
  .grouping {
    position: relative;
  }

  /* The same 30px quiet icon as the rest of the bar. Brighter once there is
     grouping to show, and held in its hover colour while the panel is open. */
  .level-icon {
    display: grid;
    place-items: center;
    width: 1.875rem;
    height: 1.875rem;
    padding: 0;
    border: 0;
    border-radius: var(--radius);
    background: none;
    color: var(--ink-2);
    cursor: pointer;
    transition:
      background var(--duration),
      color var(--duration);
  }

  .level-icon.flat {
    color: var(--ink-3);
  }

  .level-icon:hover,
  .level-icon.open {
    background: var(--paper-2);
    color: var(--ink);
  }

  /* Dressed as the context menu is: a card on the page, hung from the right
     edge of its button so it opens towards the middle of the window. */
  .panel {
    position: absolute;
    top: calc(100% + 0.375rem);
    right: 0;
    z-index: 20;
    width: 15rem;
    padding: 0.75rem 0.875rem 0.625rem;
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius);
    background: var(--card);
    box-shadow: var(--shadow-lg);
    animation: panel-in var(--duration) ease-out;
  }

  @keyframes panel-in {
    from {
      opacity: 0;
      transform: translateY(-2px);
    }
  }

  .heading {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 0.625rem;
    color: var(--ink);
    font-size: var(--fs-card);
    font-weight: 500;
  }

  .count {
    color: var(--ink-3);
    font-size: var(--fs-meta);
    font-weight: 400;
  }

  .slider {
    position: relative;
    display: block;
    height: 20px;
  }

  .groove,
  .filled {
    position: absolute;
    left: 0;
    top: 6.5px;
    height: 3px;
    border-radius: 1.5px;
  }

  .groove {
    right: 0;
    background: var(--groove);
  }

  .filled {
    background: var(--accent);
  }

  /* A ruler's marks, under the track: there to be found, not to be read. */
  .tick {
    position: absolute;
    top: 13px;
    width: 1px;
    height: 4px;
    background: var(--line-3);
    opacity: 0.7;
  }

  /* The name of each stop, centred under its tick. */
  .stops {
    position: relative;
    display: block;
    height: 0.875rem;
    margin-top: 0.1875rem;
    color: var(--ink-3);
    font-size: var(--fs-label);
  }

  .stops span {
    position: absolute;
    transform: translateX(-50%);
  }

  input {
    -webkit-appearance: none;
    appearance: none;
    position: absolute;
    inset: 0;
    width: 100%;
    height: 20px;
    margin: 0;
    background: transparent;
    cursor: pointer;
  }

  input:focus-visible {
    outline: none;
  }

  input::-webkit-slider-runnable-track {
    height: 20px;
    background: transparent;
  }

  input::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 12px;
    height: 12px;
    margin-top: 2px;
    border: 0;
    border-radius: 50%;
    background: var(--knob);
    box-shadow:
      0 0 0 0.5px var(--knob-edge),
      var(--shadow-sm);
    transition:
      transform var(--duration),
      box-shadow var(--duration);
  }

  input:active::-webkit-slider-thumb {
    transform: scale(1.15);
  }

  input:focus-visible::-webkit-slider-thumb {
    box-shadow:
      0 0 0 0.5px var(--knob-edge),
      0 0 0 3px var(--accent-wash);
  }
</style>
