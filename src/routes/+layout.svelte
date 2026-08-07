<script lang="ts">
  import '../app.css';
  import Toasts from '$lib/Toasts.svelte';

  let { children } = $props();
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
