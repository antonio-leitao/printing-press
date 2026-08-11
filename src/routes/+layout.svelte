<script lang="ts">
  import '../app.css';
  import Toasts from '$lib/Toasts.svelte';
  import { startTheme, theme } from '$lib/theme.svelte';

  let { children } = $props();

  // Before the first paint rather than in an effect: `ssr` is off, so this runs
  // in the browser and nothing is on screen yet. Seeding it after mount would
  // show the library light for a frame before a remembered dark took hold.
  startTheme();

  // The one place the attribute is written; everything else throws the switch
  // and lets it arrive here — see `$lib/theme.svelte`.
  $effect(() => {
    document.documentElement.setAttribute('data-theme', theme.chosen);
  });
</script>

<svelte:head>
  <title>Press</title>
  <meta
    name="description"
    content="A clean LaTeX project builder and PDF viewer."
  />
</svelte:head>

{@render children()}

<!-- Outside the page, so a message survives whatever the page is showing. -->
<Toasts />
