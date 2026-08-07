<script lang="ts">
  import '../app.css';

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
