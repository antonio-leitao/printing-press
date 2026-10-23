<script lang="ts">
  /* Keeping a project as stationery: the name it will be chosen by, and which
     of its files go with it.

     The listing is fetched when the dialog opens and is Press's own walk, so
     what can be ticked is exactly what a version would hold. `chosen` is a set
     of the same paths, and the document's own is in it and cannot leave. */

  import { onMount } from 'svelte';
  import { Check, Folder, Minus } from '@lucide/svelte';
  import { api } from '$lib/api';
  import { fail, notify } from '$lib/messages.svelte';
  import { modal } from '$lib/modal';
  import type { ProjectFile, ProjectSummary, TemplateScope } from '$lib/types';

  let {
    project,
    busy = false,
    onclose
  }: {
    project: ProjectSummary;
    busy?: boolean;
    onclose: () => void;
  } = $props();

  // The document's own name without its extension. The project's name carries
  // the folder above it, which is what tells two `main.tex`es apart in a
  // library and is not what a template wants to be called.
  // svelte-ignore state_referenced_locally
  let templateName = $state(project.fileName.replace(/\.[^.]+$/, ''));
  let templateScope = $state<TemplateScope>('document');
  let projectFiles = $state<ProjectFile[]>([]);
  // svelte-ignore state_referenced_locally
  let chosenFiles = $state<Set<string>>(new Set([project.fileName]));
  // Fetched with the dialog rather than when the tree is asked for, so
  // choosing to select files shows them rather than a wait.
  let listingFiles = $state(true);
  let saving = $state(false);
  const working = $derived(busy || saving);

  onMount(async () => {
    try {
      projectFiles = await api.projectFiles(project.id);
    } catch (reason) {
      fail(reason);
    } finally {
      listingFiles = false;
    }
  });

  /// The listing as a tree, flattened to rows.
  ///
  /// Paths sort so that a folder's files are contiguous, which is the whole
  /// trick: one pass emits a folder row the first time a prefix appears and a
  /// file row for every file, each carrying the depth to indent by. A tree
  /// drawn without a recursive component, and one that can be walked to work
  /// out what a folder's tick should say.
  const fileRows = $derived.by(() => {
    const sorted = [...projectFiles].sort((a, b) => a.path.localeCompare(b.path));
    const rows: Array<{
      kind: 'folder' | 'file';
      path: string;
      name: string;
      depth: number;
      byteSize: number;
      /** Every file under this row: itself, or a folder's descendants. */
      files: string[];
    }> = [];
    const seen = new Set<string>();

    for (const file of sorted) {
      const parts = file.path.split('/');
      for (let depth = 0; depth < parts.length - 1; depth += 1) {
        const prefix = parts.slice(0, depth + 1).join('/');
        if (seen.has(prefix)) continue;
        seen.add(prefix);
        const under = sorted.filter((one) => one.path.startsWith(`${prefix}/`));
        rows.push({
          kind: 'folder',
          path: prefix,
          name: parts[depth],
          depth,
          byteSize: under.reduce((total, one) => total + one.byteSize, 0),
          files: under.map((one) => one.path)
        });
      }
      rows.push({
        kind: 'file',
        path: file.path,
        name: parts[parts.length - 1],
        depth: parts.length - 1,
        byteSize: file.byteSize,
        files: [file.path]
      });
    }
    return rows;
  });

  const chosenBytes = $derived(
    projectFiles
      .filter((file) => chosenFiles.has(file.path))
      .reduce((total, file) => total + file.byteSize, 0)
  );

  /// The document alone, or all of it and then strike things out. Starting the
  /// tree fully ticked is the honest default for a choice named "select files":
  /// what is on screen when it opens is what will be kept if nothing is touched.
  function chooseScope(scope: TemplateScope) {
    templateScope = scope;
    chosenFiles =
      scope === 'document'
        ? new Set([project.fileName])
        : new Set(projectFiles.map((file) => file.path));
  }

  /// The document's own file cannot be struck out: a template without it is not
  /// a template, and the backend refuses one for the same reason.
  function locked(path: string) {
    return path === project.fileName;
  }

  /// A folder is ticked when everything under it is. Half of it is neither, and
  /// says so with a dash rather than by rounding to one of the two.
  function tickState(files: string[]): 'all' | 'some' | 'none' {
    const held = files.filter((path) => chosenFiles.has(path)).length;
    if (held === 0) return 'none';
    return held === files.length ? 'all' : 'some';
  }

  /// Reassigned rather than mutated: a `Set` changed in place is the same `Set`,
  /// and nothing downstream would hear about it.
  function toggleFiles(files: string[]) {
    const next = new Set(chosenFiles);
    if (tickState(files) === 'all') {
      for (const path of files) if (!locked(path)) next.delete(path);
    } else {
      for (const path of files) next.add(path);
    }
    chosenFiles = next;
  }

  /// Bytes as a person reads them. Only ever describing a selection, so one
  /// decimal on the way up and none below a kilobyte.
  function readableSize(bytes: number) {
    if (bytes < 1024) return `${bytes} B`;
    const units = ['kB', 'MB', 'GB'];
    let value = bytes / 1024;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit += 1;
    }
    return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
  }

  async function saveTemplate() {
    if (!templateName.trim() || chosenFiles.size === 0) return;
    saving = true;
    try {
      await api.saveAsTemplate(project.id, templateName.trim(), [...chosenFiles]);
      notify(`Saved “${templateName.trim()}” as a template.`);
      onclose();
    } catch (reason) {
      fail(reason);
    } finally {
      saving = false;
    }
  }
</script>

<dialog use:modal={onclose} aria-labelledby="template-title">
  <h2 id="template-title">Save as template</h2>
  <p class="quiet">
    A copy of {project.fileName} as it stands now. The project is untouched, and editing it
    later does not change the template.
  </p>
  <label>
    Name
    <input
      bind:value={templateName}
      maxlength="60"
      onkeydown={(event) => {
        if (event.key === 'Enter') void saveTemplate();
      }}
    />
  </label>

  <!-- Two answers rather than a list of files to tick. There are two intents
       here — a house style, and a project you want to begin from again — and
       ticking files serves neither. -->
  <div class="scopes" role="radiogroup" aria-label="What the template keeps">
    <button
      class="scope"
      role="radio"
      aria-checked={templateScope === 'document'}
      onclick={() => chooseScope('document')}
    >
      <strong>The document alone</strong>
      <span class="quiet">One file, whatever else is in the folder with it.</span>
    </button>
    <button
      class="scope"
      role="radio"
      aria-checked={templateScope === 'project'}
      onclick={() => chooseScope('project')}
      disabled={listingFiles}
    >
      <strong>Select files</strong>
      <span class="quiet">
        {#if listingFiles}
          Reading the folder…
        {:else}
          Everything it compiles with, minus whatever you strike out.
        {/if}
      </span>
    </button>
  </div>

  {#if templateScope === 'project'}
    <!-- Only what a version would hold: build output, editor scratch, `.git`
         and a neighbour's document are not in the listing at all, so they
         cannot be ticked back in. The document's own row is ticked and
         locked — a template without it is not a template. -->
    <div class="tree" role="group" aria-label="Files to keep">
      {#each fileRows as row (row.path)}
        {@const state = tickState(row.files)}
        {@const fixed = row.kind === 'file' && locked(row.path)}
        <button
          class="tree-row"
          class:folder={row.kind === 'folder'}
          class:fixed
          style="padding-left: calc({row.depth} * 1.1rem + 0.4rem)"
          aria-pressed={state !== 'none'}
          disabled={fixed}
          title={fixed ? 'The document itself is always kept' : row.path}
          onclick={() => toggleFiles(row.files)}
        >
          <span class="tick" class:on={state !== 'none'}>
            {#if state === 'all'}
              <Check size={11} strokeWidth={3} aria-hidden="true" />
            {:else if state === 'some'}
              <Minus size={11} strokeWidth={3} aria-hidden="true" />
            {/if}
          </span>
          {#if row.kind === 'folder'}
            <Folder size={13} strokeWidth={1.75} aria-hidden="true" />
          {/if}
          <span class="tree-name">{row.name}</span>
          <span class="tree-size">{readableSize(row.byteSize)}</span>
        </button>
      {/each}
    </div>

    <p class="quiet tree-total">
      {chosenFiles.size}
      {chosenFiles.size === 1 ? 'file' : 'files'}, {readableSize(chosenBytes)}
    </p>
  {/if}

  <div class="dialog-actions">
    <button onclick={onclose} disabled={working}>Cancel</button>
    <button onclick={saveTemplate} disabled={working || !templateName.trim() || chosenFiles.size === 0}>
      {working ? 'Saving…' : 'Save'}
    </button>
  </div>
</dialog>

<style>
  .quiet {
    color: var(--ink-3);
  }

  /* A listing rather than a control panel: no rules between rows, no boxes
     around names, and the indent alone saying what is inside what. The frame is
     the field's own, because that is what this is — one field holding a set. */
  .tree {
    max-height: 15rem;
    overflow-y: auto;
    padding: 0.25rem 0;
    margin-bottom: var(--space-sm);
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius-sm);
    background: var(--paper);
  }

  .tree-row {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    width: 100%;
    padding-block: 0.22rem;
    padding-right: 0.6rem;
    border: 0;
    background: none;
    color: var(--ink-2);
    font-family: var(--font-mono);
    font-size: var(--fs-meta);
    line-height: 1.5;
    text-align: left;
    cursor: pointer;
    transition: background var(--duration);
  }

  .tree-row:hover:not(:disabled) {
    background: var(--paper-2);
  }

  /* A folder is the thing its contents are named after, so it is set in the
     interface's own face rather than the face the paths are in. */
  .tree-row.folder {
    color: var(--ink-3);
    font-family: var(--font-sans);
  }

  /* The document itself, which cannot be struck out. Dimmed rather than
     disabled-looking: it is not unavailable, it is settled. */
  .tree-row.fixed {
    cursor: default;
    opacity: 0.75;
  }

  .tree-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Right-aligned and quiet: a column to be scanned when the question is what
     is making this template large, and ignorable the rest of the time. */
  .tree-size {
    flex: none;
    color: var(--ink-3);
    font-family: var(--font-mono);
    font-size: var(--fs-meta);
    font-variant-numeric: tabular-nums;
  }

  /* The square a radio's dot would be if this were a choice of one. Same size,
     same ring, same accent when it is on — square because these are not
     alternatives and the shape is the only thing that says so. */
  .tick {
    flex: none;
    display: grid;
    place-items: center;
    width: 0.85rem;
    height: 0.85rem;
    border: var(--bw) solid var(--line-3);
    border-radius: var(--radius-xs);
    background: transparent;
    color: var(--on-accent);
    transition:
      border-color var(--duration),
      background var(--duration);
  }

  .tick.on {
    border-color: var(--accent);
    background: var(--accent);
  }

  .tree-total {
    margin-bottom: var(--space-lg);
    font-size: var(--fs-meta);
    font-variant-numeric: tabular-nums;
  }
</style>
