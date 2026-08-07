<script lang="ts">
  import { onMount, type Component } from 'svelte';
  import { Download, Pencil, Pin, PinOff, Trash2 } from '@lucide/svelte';
  import { listen } from '@tauri-apps/api/event';
  import { open } from '@tauri-apps/plugin-dialog';
  import PdfThumbnail from '$lib/PdfThumbnail.svelte';
  import PdfViewer from '$lib/PdfViewer.svelte';
  import { api, errorMessage } from '$lib/api';
  import {
    ENGINES,
    WORKTREE,
    type BuildProgress,
    type BuildUpdate,
    type Engine,
    type OpenCandidate,
    type OpenRequest,
    type ProjectSummary,
    type VersionSummary,
    type WatcherError
  } from '$lib/types';

  /** Matches the `.pinned-card` track width below, so nothing larger is drawn. */
  const PINNED_THUMB_WIDTH = 150
  /** Likewise for `.project-thumb`. */
  const GRID_THUMB_WIDTH = 55

  const KEY_HELP: Array<[string, string]> = [
    ['j / k', 'scroll down / up'],
    ['h / l', 'scroll left / right'],
    ['d / u', 'half page down / up'],
    ['f / b / space', 'page down / up'],
    ['J / K', 'next / previous page'],
    ['gg / G', 'first / last page'],
    ['12G', 'go to page 12'],
    ['+ / -', 'zoom in / out'],
    ['0', 'actual size'],
    ['a / s', 'fit page / fit width'],
    ['R', 'rebuild this version'],
    ['⌘K', 'snapshot the working tree'],
    ['?', 'this list']
  ];

  let projects = $state<ProjectSummary[]>([]);
  let activeProject = $state<ProjectSummary | null>(null);
  /** Documents a path resolved to, shown when there is a choice to make. */
  let choosing = $state<OpenRequest | null>(null);
  let chosen = $state('');
  let chosenEngine = $state<Engine>('pdflatex');
  let busy = $state(false);
  let error = $state('');
  let notice = $state('');
  let watcherNotice = $state('');
  let buildLog = $state('');
  let editorState = $state('closed');
  let progress = $state<BuildProgress | null>(null);
  let settingsFor = $state<ProjectSummary | null>(null);
  let settingsName = $state('');
  let settingsEngine = $state<Engine>('pdflatex');
  let confirmDelete = $state<ProjectSummary | null>(null);

  // -- history -------------------------------------------------------------
  let versions = $state<VersionSummary[]>([]);
  /** Which version the viewer is showing. */
  let selectedRef = $state<string>(WORKTREE);
  let showHistory = $state(false);
  let snapshotOpen = $state(false);
  let snapshotTitle = $state('');
  let snapshotBody = $state('');
  let renaming = $state<VersionSummary | null>(null);
  let renameTitle = $state('');

  const selected = $derived(
    versions.find((version) => version.sourceRef === selectedRef) ?? versions[0] ?? null
  );
  /** The working tree's artifact is the fallback while a version is still building. */
  const shownArtifact = $derived(selected?.artifact ?? null);

  // Viewer state, surfaced in the footer.
  let viewerPage = $state(1);
  let viewerPageCount = $state(0);
  let viewerZoom = $state(100);
  let viewerError = $state('');

  type Panel = 'none' | 'diagnostics' | 'log' | 'keys';
  let panel = $state<Panel>('none');

  // -- context menu ---------------------------------------------------------
  type MenuItem = {
    label: string;
    icon: Component;
    run: () => void;
    disabled?: boolean;
    /// Destructive: red on hover, and cut off from the items above by a rule.
    danger?: boolean;
  };
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  /// Opens the menu where it was asked for. Right-click and the `⋯` button both
  /// come here, so there is one list of actions per thing rather than two.
  function openMenu(event: MouseEvent, items: MenuItem[]) {
    event.preventDefault();
    event.stopPropagation();
    menu = { x: event.clientX, y: event.clientY, items };
  }

  function runMenuItem(item: MenuItem) {
    menu = null;
    item.run();
  }

  function projectMenu(project: ProjectSummary): MenuItem[] {
    return [
      {
        label: project.pinned ? 'Unpin' : 'Pin to top',
        icon: project.pinned ? PinOff : Pin,
        run: () => void setPinned(project, !project.pinned)
      },
      { label: 'Rename…', icon: Pencil, run: () => openSettings(project) },
      {
        label: 'Download PDF',
        icon: Download,
        run: () => void downloadArtifact(project.artifact?.id),
        disabled: !project.artifact
      },
      {
        label: 'Remove…',
        icon: Trash2,
        danger: true,
        run: () => (confirmDelete = project)
      }
    ];
  }

  async function setPinned(project: ProjectSummary, pinned: boolean) {
    clearMessages();
    try {
      await api.setProjectPinned(project.id, pinned);
      // Refetched rather than merged: pinning changes the order, and the
      // backend is what decides the order.
      await refreshProjects();
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  function versionMenu(version: VersionSummary): MenuItem[] {
    const items: MenuItem[] = [
      {
        label: 'Download PDF',
        icon: Download,
        run: () => void downloadArtifact(version.artifact?.id),
        disabled: !version.artifact
      }
    ];
    // The working tree is not a stored version, so it has neither of these.
    if (version.snapshot) {
      items.unshift({
        label: 'Rename…',
        icon: Pencil,
        run: () => {
          renaming = version;
          renameTitle = version.title;
        }
      });
      items.push({
        label: 'Discard',
        icon: Trash2,
        danger: true,
        run: () => void discardVersion(version)
      });
    }
    return items;
  }

  async function downloadArtifact(artifactId: number | undefined) {
    if (artifactId === undefined) return;
    clearMessages();
    try {
      notice = `Saved ${await api.exportArtifact(artifactId)}`;
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  const chosenCandidate = $derived(
    choosing?.candidates.find((candidate) => candidate.documentPath === chosen) ?? null
  );

  // Two sections, one list: the backend already returns pinned first, so
  // splitting it here keeps both in the order it decided.
  const pinned = $derived(projects.filter((project) => project.pinned));
  const unpinned = $derived(projects.filter((project) => !project.pinned));

  const dialogOpen = $derived(
    Boolean(choosing || settingsFor || confirmDelete || snapshotOpen || renaming || menu)
  );

  onMount(() => {
    const unlisteners: Array<() => void> = [];
    let disposed = false;

    void (async () => {
      unlisteners.push(
        await listen<ProjectSummary>('project-updated', (event) => mergeProject(event.payload))
      );
      unlisteners.push(
        await listen<BuildUpdate>('build-updated', (event) => {
          const update = event.payload;
          if (activeProject?.id !== update.projectId) return;
          // Every version's row carries its own build state.
          versions = versions.map((version) =>
            version.sourceRef === update.sourceRef
              ? { ...version, build: update.build, artifact: update.artifact ?? version.artifact }
              : version
          );
          if (update.sourceRef !== selectedRef) return;
          if (update.build.status !== 'running' && update.build.status !== 'queued') {
            progress = null;
          }
          if (update.build.status !== 'running') buildLog = '';
        })
      );
      unlisteners.push(
        await listen<BuildProgress>('build-progress', (event) => {
          if (event.payload.projectId !== activeProject?.id) return;
          if (event.payload.sourceRef !== selectedRef) return;
          progress = event.payload;
        })
      );
      unlisteners.push(
        await listen<WatcherError>('watcher-error', (event) => {
          // Not a build failure: the document is fine, Press just cannot see saves.
          watcherNotice = event.payload.message;
        })
      );
      unlisteners.push(
        // Only a nudge: the request itself is collected, so a missed event or a
        // webview that was not listening yet costs nothing.
        await listen('open-requested', () => void collectPendingOpen())
      );
      if (!disposed) {
        // Before the project list, so a database that was set aside explains
        // why the list is empty.
        try {
          const startup = await api.takeStartupNotice();
          if (startup) notice = startup;
        } catch {
          // A missing notice is not worth reporting.
        }
        await refreshProjects();
        await collectPendingOpen();
      }
    })();

    const shortcuts = (event: KeyboardEvent) => {
      // Before the guards below, which treat an open menu as a dialog.
      if (menu && event.key === 'Escape') {
        menu = null;
        return;
      }
      if (!activeProject || dialogOpen) return;
      const target = event.target;
      const typing =
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target instanceof HTMLElement && target.isContentEditable);
      if (typing) return;

      // `code` as well as `key`, because a layout that does not put `k` on the
      // physical K key reports something else for `key` under a modifier.
      const isK = event.code === 'KeyK' || event.key.toLowerCase() === 'k';
      if ((event.metaKey || event.ctrlKey) && isK) {
        event.preventDefault();
        openSnapshotDialog();
        return;
      }
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      // Rebuilding is rarely needed — saves do it — so it is a key, not a button.
      if (event.key === 'R') {
        event.preventDefault();
        void rebuild();
      } else if (event.key === '?') {
        event.preventDefault();
        void togglePanel('keys');
      }
    };
    window.addEventListener('keydown', shortcuts);

    // A cheap socket probe, so this cadence costs almost nothing.
    const editorPoll = window.setInterval(() => {
      const id = activeProject?.id;
      if (id === undefined) return;
      void api
        .editorStatus(id)
        .then((status) => (editorState = status))
        .catch(() => (editorState = 'closed'));
    }, 5000);

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
      window.removeEventListener('keydown', shortcuts);
      window.clearInterval(editorPoll);
    };
  });

  function mergeProject(project: ProjectSummary) {
    const index = projects.findIndex((item) => item.id === project.id);
    projects =
      index === -1
        ? [project, ...projects]
        : projects.map((item) => (item.id === project.id ? project : item));
    if (activeProject?.id === project.id) activeProject = project;
  }

  async function refreshProjects() {
    try {
      projects = await api.listProjects();
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  /// Point at the document. A folder is never a project, so there is nothing to
  /// pick a folder for: `:Press` and `press <path>` can still hand over a
  /// directory, and it opens the same picker this does.
  async function chooseDocument() {
    clearMessages();
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        title: 'Open a document',
        filters: [
          { name: 'Documents', extensions: ['tex', 'ltx', 'Rnw', 'md', 'markdown', 'qmd', 'mkd'] }
        ]
      });
      if (!selected || Array.isArray(selected)) return;
      busy = true;
      await present(await api.resolvePath(selected));
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  /// Acts on what a path resolved to. One candidate and nothing to warn about
  /// opens it; anything else asks. Shared by every way into Press.
  async function present(request: OpenRequest) {
    if (!request.toolchain.latexmk.available) {
      error = 'latexmk was not found. Install a TeX distribution or add latexmk to PATH.';
      return;
    }
    if (request.candidates.length === 0) {
      error = request.warnings[0] ?? `Press found no document in ${request.path}.`;
      return;
    }
    const only = request.candidates.length === 1 ? request.candidates[0] : null;
    // A `.latexmkrc` is executable Perl, so it is shown before it ever runs —
    // but only once: a document Press already keeps was accepted when it was
    // added, and asking again on every `:Press` would train the answer away.
    const consented = only?.projectId !== null || only.latexmkrcPaths.length === 0;
    if (only && consented && request.warnings.length === 0) {
      await openCandidate(only);
      return;
    }
    choosing = request;
    selectCandidate(only?.documentPath ?? request.candidates[0].documentPath);
  }

  /// The engine follows the document: it is detected per document, so it cannot
  /// stay behind on the previous one's.
  function selectCandidate(documentPath: string) {
    chosen = documentPath;
    chosenEngine =
      choosing?.candidates.find((candidate) => candidate.documentPath === documentPath)?.engine ??
      'pdflatex';
  }

  /// Collects a path Press was launched with, from `:Press` or the command line.
  async function collectPendingOpen() {
    try {
      const request = await api.takePendingOpen();
      if (!request) return;
      clearMessages();
      busy = true;
      try {
        await present(request);
      } finally {
        busy = false;
      }
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  async function confirmChoice() {
    if (!chosenCandidate) return;
    busy = true;
    clearMessages();
    try {
      await openCandidate(chosenCandidate, chosenEngine);
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  /// Opens a document, keeping it first if Press does not have it yet. Adding a
  /// document it already has just touches it, so both paths are one call.
  async function openCandidate(candidate: OpenCandidate, engine?: Engine) {
    const project = await api.addProject(
      candidate.documentPath,
      candidate.name,
      engine ?? candidate.engine ?? undefined
    );
    choosing = null;
    chosen = '';
    projects = [project, ...projects.filter((item) => item.id !== project.id)];
    await openProject(project);
  }

  async function openProject(project: ProjectSummary) {
    clearMessages();
    busy = true;
    try {
      activeProject = await api.openProject(project.id);
      editorState = await api.editorStatus(project.id);
      panel = 'none';
      selectedRef = WORKTREE;
      await refreshVersions();
    } catch (reason) {
      error = errorMessage(reason);
      activeProject = null;
    } finally {
      busy = false;
    }
  }

  async function returnToLibrary() {
    busy = true;
    try {
      await api.closeProject();
      activeProject = null;
      editorState = 'closed';
      buildLog = '';
      progress = null;
      watcherNotice = '';
      panel = 'none';
      versions = [];
      showHistory = false;
      await refreshProjects();
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  async function refreshVersions() {
    if (!activeProject) return;
    try {
      versions = await api.listVersions(activeProject.id);
      if (!versions.some((version) => version.sourceRef === selectedRef)) {
        selectedRef = WORKTREE;
      }
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  function openSnapshotDialog() {
    if (!activeProject) return;
    snapshotTitle = '';
    snapshotBody = '';
    snapshotOpen = true;
  }

  async function takeSnapshot() {
    if (!activeProject || !snapshotTitle.trim()) return;
    busy = true;
    clearMessages();
    try {
      const snapshot = await api.createSnapshot(
        activeProject.id,
        snapshotTitle.trim(),
        snapshotBody.trim() || undefined
      );
      snapshotOpen = false;
      await refreshVersions();
      // Show what was just stored; it is already building.
      selectedRef = `snapshot:${snapshot.revision}`;
      showHistory = true;
      notice = `Stored “${snapshot.title}” — ${snapshot.fileCount} files.`;
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  /** Shows a version, building it first if it has never been compiled. */
  async function selectVersion(version: VersionSummary) {
    selectedRef = version.sourceRef;
    progress = null;
    buildLog = '';
    if (version.artifact || !activeProject) return;
    if (version.build.status === 'running' || version.build.status === 'queued') return;
    try {
      await api.buildProject(activeProject.id, version.sourceRef);
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  async function saveRename() {
    const version = renaming;
    if (!version?.snapshot || !renameTitle.trim()) return;
    busy = true;
    try {
      await api.renameSnapshot(version.snapshot.id, renameTitle.trim());
      renaming = null;
      await refreshVersions();
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  async function discardVersion(version: VersionSummary) {
    if (!version.snapshot) return;
    busy = true;
    clearMessages();
    try {
      await api.deleteSnapshot(version.snapshot.id);
      if (selectedRef === version.sourceRef) selectedRef = WORKTREE;
      await refreshVersions();
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  /// A grid card's last line: when the PDF under the thumbnail was made, and
  /// how much history there is behind it. An artifact wins over a failed build,
  /// because that older PDF is what the thumbnail is showing.
  ///
  /// Kept to the sidebar's shorthand — `2h ago`, not `Built 2h ago` — because
  /// the column is 137px wide at the default window and the version count has
  /// to fit beside it.
  function cardMeta(project: ProjectSummary) {
    const { status } = project.build;
    const building = status === 'queued' || status === 'running';
    const state = building
      ? 'building…'
      : age(project.artifact?.builtAt) ||
        (status === 'error' ? 'does not compile' : 'never built');
    const versions = project.snapshotCount;
    if (versions === 0) return state;
    return `${state} · ${versions} version${versions === 1 ? '' : 's'}`;
  }

  function age(seconds: number | null | undefined) {
    if (!seconds) return '';
    const elapsed = Math.max(0, Math.floor(Date.now() / 1000 - seconds));
    if (elapsed < 60) return 'just now';
    if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
    if (elapsed < 86400) return `${Math.floor(elapsed / 3600)}h ago`;
    return `${Math.floor(elapsed / 86400)}d ago`;
  }

  /** What the sidebar says about a version's build. */
  function versionState(version: VersionSummary) {
    if (version.build.status === 'running' || version.build.status === 'queued') {
      return 'building';
    }
    if (version.build.status === 'error') return 'fails to compile';
    return version.artifact ? 'built' : 'not built';
  }

  async function rebuild() {
    if (!activeProject) return;
    clearMessages();
    try {
      await api.buildProject(activeProject.id, selectedRef);
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  async function launchEditor() {
    if (!activeProject) return;
    clearMessages();
    try {
      const result = await api.launchNeovim(activeProject.id);
      notice = result.message;
      editorState = result.status === 'connected' ? 'connected' : 'starting';
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  function openSettings(project: ProjectSummary) {
    settingsFor = project;
    settingsName = project.name;
    settingsEngine = project.engine;
  }

  async function saveSettings() {
    const project = settingsFor;
    if (!project) return;
    busy = true;
    clearMessages();
    try {
      const name = settingsName.trim();
      if (name && name !== project.name) {
        mergeProject(await api.renameProject(project.id, name));
      }
      if (settingsEngine !== project.engine) {
        // Discards every cached PDF. The new build produces a new artifact id,
        // so nothing stale can be shown.
        mergeProject(await api.setProjectEngine(project.id, settingsEngine));
        notice = 'Engine changed. Cached PDFs were discarded and a rebuild has started.';
      }
      settingsFor = null;
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  async function removeProject() {
    const project = confirmDelete;
    if (!project) return;
    busy = true;
    clearMessages();
    try {
      await api.deleteProject(project.id);
      projects = projects.filter((item) => item.id !== project.id);
      if (activeProject?.id === project.id) {
        activeProject = null;
        buildLog = '';
        progress = null;
      }
      confirmDelete = null;
      notice = `Removed ${project.name} from Press. The document itself was not touched.`;
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  async function togglePanel(next: Panel) {
    panel = panel === next ? 'none' : next;
    if (panel === 'log' && activeProject && !buildLog) {
      try {
        buildLog = await api.getBuildLog(activeProject.id);
      } catch (reason) {
        buildLog = errorMessage(reason);
      }
    }
  }

  function clearMessages() {
    error = '';
    notice = '';
  }

  function statusTone(project: ProjectSummary) {
    const { status } = selected?.build ?? project.build;
    if (status === 'success') return 'good';
    if (status === 'error') return 'bad';
    if (status === 'running' || status === 'queued') return 'busy';
    return 'idle';
  }

  /// What the footer says: which version, and whether it is current.
  function versionLabel() {
    const version = selected;
    const name = version && version.sourceRef !== WORKTREE ? version.title : 'Working tree';
    const build = version?.build ?? activeProject?.build;
    if (!build) return name;

    if (build.status === 'queued') return `${name} · queued`;
    if (build.status === 'running') {
      if (progress) {
        const page = progress.page
          ? progress.expectedPages
            ? ` · page ${progress.page} of ~${progress.expectedPages}`
            : ` · page ${progress.page}`
          : '';
        return `${name} · compiling${page}`;
      }
      return `${name} · compiling`;
    }
    if (build.status === 'error') return `${name} · does not compile`;
    if (build.status === 'interrupted') return `${name} · interrupted`;
    if (build.status === 'never') return `${name} · not built`;
    return version && version.sourceRef !== WORKTREE ? name : `${name} · up to date`;
  }

  function location(file: string | null, line: number | null) {
    if (!file) return '';
    return line ? `${file}:${line}` : file;
  }

  const errors = $derived(
    activeProject?.build.diagnostics.filter((item) => item.severity === 'error') ?? []
  );
  const warnings = $derived(
    activeProject?.build.diagnostics.filter((item) => item.severity === 'warning') ?? []
  );
  const transient = $derived(error || viewerError || watcherNotice || notice);
</script>

<!-- What a document is, shared by the shelf and the grid so it reads the same
     wherever it is shown. One word, so it still fits a 150px shelf card once a
     gone document has flagged itself. -->
{#snippet kindLine(project: ProjectSummary)}
  <span class="quiet kind"
    >{project.kind}{#if !project.available} · <span class="bad">missing</span>{/if}</span
  >
{/snippet}

{#if activeProject}
  <main class="reader">
    <!-- Not a bar: this takes no space in the layout. It sits over the grey
         gutter beside the traffic lights purely so the window can be dragged. -->
    <div class="drag-zone" data-tauri-drag-region></div>

    <section class="document">
      {#if showHistory}
        <nav class="history" aria-label="Version history">
          <!-- Clears the traffic lights, which sit over this panel's top left
               whenever it is open. -->
          <div class="history-head quiet">{activeProject.name}</div>
          {#each versions as version (version.sourceRef)}
            <div class="version" class:current={version.sourceRef === selectedRef}>
              <!-- Right-click for rename, download and discard. -->
              <button
                class="version-open"
                onclick={() => selectVersion(version)}
                oncontextmenu={(event) => openMenu(event, versionMenu(version))}
              >
                <strong>{version.title}</strong>
                <span class="quiet">
                  {version.snapshot ? age(version.snapshot.createdAt) : 'live'}
                  · {versionState(version)}
                </span>
                {#if version.snapshot?.body}
                  <span class="quiet body">{version.snapshot.body}</span>
                {/if}
              </button>
            </div>
          {/each}
        </nav>
      {/if}

      <div class="stage">
        {#if shownArtifact}
          <PdfViewer
            artifact={shownArtifact}
            bind:page={viewerPage}
            bind:pageCount={viewerPageCount}
            bind:zoomPercent={viewerZoom}
            bind:loadError={viewerError}
            enabled={!dialogOpen}
          />
        {:else}
          <div class="empty">
            {#if selected && (selected.build.status === 'running' || selected.build.status === 'queued')}
              <p>Building this version…</p>
            {:else if selected && selected.build.status === 'error'}
              <p>This version does not compile.</p>
            {:else}
              <p>No PDF for this version yet.</p>
            {/if}
          </div>
        {/if}
      </div>
    </section>

    {#if panel !== 'none'}
      <section class="panel">
        {#if panel === 'diagnostics'}
          <button class="link" onclick={() => togglePanel('log')}>Build output ›</button>
          {#if errors.length + warnings.length === 0}
            <p class="quiet">No errors or warnings in the last build.</p>
          {:else}
            <ul class="diagnostics">
              {#each [...errors, ...warnings] as diagnostic}
                <li class={diagnostic.severity}>
                  {#if diagnostic.file}<code>{location(diagnostic.file, diagnostic.line)}</code>{/if}
                  <span>{diagnostic.message}</span>
                </li>
              {/each}
            </ul>
          {/if}
        {:else if panel === 'log'}
          <pre>{buildLog || 'No build output yet.'}</pre>
        {:else if panel === 'keys'}
          <dl class="keys">
            {#each KEY_HELP as [keys, meaning]}
              <dt><kbd>{keys}</kbd></dt>
              <dd>{meaning}</dd>
            {/each}
          </dl>
        {/if}
      </section>
    {/if}

    {#if transient}
      <p class="strip" class:bad={Boolean(error || viewerError || watcherNotice)} role="status">
        <span>{error || viewerError || watcherNotice || notice}</span>
        <button
          class="link"
          onclick={() => {
            error = '';
            notice = '';
            watcherNotice = '';
            viewerError = '';
          }}
        >
          ✕
        </button>
      </p>
    {/if}

    <footer class="bar bottom">
      <button class="link" onclick={() => (showHistory = !showHistory)} title="Version history">
        {showHistory ? '◀' : '☰'}
      </button>
      <span class="dot {statusTone(activeProject)}"></span>
      <span class="build">{versionLabel()}</span>
      {#if errors.length > 0}
        <button class="link bad" onclick={() => togglePanel('diagnostics')}>
          {errors.length} error{errors.length === 1 ? '' : 's'}
        </button>
      {/if}
      <span class="spacer"></span>
      {#if viewerPageCount > 0}
        <span class="quiet">{viewerPage}/{viewerPageCount}</span>
      {/if}
      {#if viewerZoom !== 100}<span class="quiet">{viewerZoom}%</span>{/if}
      <!-- ⌘K as well, but a keystroke nobody can see is not a way to reach a
           feature, and it depends on the webview getting the key at all. -->
      <button class="link" onclick={openSnapshotDialog} disabled={busy} title="Snapshot (⌘K)">
        Snapshot
      </button>
      <button class="link" onclick={launchEditor} disabled={busy}>Editor</button>
      <button class="link" onclick={returnToLibrary} disabled={busy}>Projects</button>
      <button class="link" onclick={() => togglePanel('keys')} title="Keys (?)">?</button>
    </footer>
  </main>
{:else}
  <main class="library">
    <div class="titlebar" data-tauri-drag-region></div>
    <header class="library-header">
      <div>
        <h1>Printing Press</h1>
        <p class="quiet">Documents and their last successful builds</p>
      </div>
      <div class="library-actions">
        <button onclick={chooseDocument} disabled={busy}>
          {busy ? 'Opening…' : 'Open document'}
        </button>
      </div>
    </header>

    {#if error}<p class="strip bad" role="alert">{error}</p>{/if}
    {#if notice}<p class="strip" role="status">{notice}</p>{/if}

    {#if projects.length === 0}
      <section class="empty-library">
        <h2>No documents yet</h2>
        <p>
          Point Press at a document — a <code>.tex</code> or a <code>.md</code> — and it compiles
          that document. Several documents can share a folder and each is its own project, with
          its own history. Naming a chapter opens the paper that includes it. Press writes nothing
          into the folder.
        </p>
        <div class="library-actions">
          <button onclick={chooseDocument} disabled={busy}>Open document</button>
        </div>
      </section>
    {:else}
      {#if pinned.length > 0}
        <section class="pinned-row" aria-label="Pinned documents">
          {#each pinned as project (project.id)}
            <article class="pinned-card">
              <button
                class="pinned-open"
                onclick={() => openProject(project)}
                oncontextmenu={(event) => openMenu(event, projectMenu(project))}
                disabled={busy || !project.available}
              >
                <span class="pinned-thumb">
                  {#if project.artifact}
                    {#key project.artifact.revision}
                      <PdfThumbnail artifact={project.artifact} width={PINNED_THUMB_WIDTH} />
                    {/key}
                  {:else}
                    <span class="missing-thumbnail">No PDF</span>
                  {/if}
                </span>
                <span class="pinned-lines">
                  <strong class="name" title={project.documentPath}>{project.name}</strong>
                  {@render kindLine(project)}
                </span>
              </button>
            </article>
          {/each}
        </section>
      {/if}

      <section class="project-grid" aria-label="Saved documents">
        {#each unpinned as project (project.id)}
          <article class="project-card">
            <!-- Right-click anywhere on the row for pin, rename, download and
                 remove. There is no ⋯ button: the menu is the only place those
                 live, so there is nothing to keep in step with it. -->
            <button
              class="project-open"
              onclick={() => openProject(project)}
              oncontextmenu={(event) => openMenu(event, projectMenu(project))}
              disabled={busy || !project.available}
            >
              <span class="project-thumb">
                {#if project.artifact}
                  {#key project.artifact.revision}
                    <PdfThumbnail artifact={project.artifact} width={GRID_THUMB_WIDTH} />
                  {/key}
                {:else}
                  <span class="missing-thumbnail">No PDF</span>
                {/if}
              </span>
              <span class="project-lines">
                <strong class="name" title={project.documentPath}>{project.name}</strong>
                {@render kindLine(project)}
                <span class="quiet when">{cardMeta(project)}</span>
              </span>
            </button>
          </article>
        {/each}
      </section>
    {/if}
  </main>
{/if}

{#if menu}
  <!-- A button rather than a div: it is a real click target, and Escape is
       handled with the other shortcuts. -->
  <button class="menu-backdrop" aria-label="Close menu" onclick={() => (menu = null)}></button>
  <menu class="context-menu" style="left: {menu.x}px; top: {menu.y}px">
    {#each menu.items as item}
      {@const Icon = item.icon}
      <li class:danger={item.danger}>
        <button onclick={() => runMenuItem(item)} disabled={item.disabled}>
          <Icon size={15} strokeWidth={1.75} aria-hidden="true" />
          {item.label}
        </button>
      </li>
    {/each}
  </menu>
{/if}

{#if choosing}
  <dialog open aria-labelledby="choose-title">
    <h2 id="choose-title">
      {choosing.candidates.length > 1 ? 'Which document?' : 'Open this document'}
    </h2>
    {#if choosing.candidates.length > 1}
      <p>Press found {choosing.candidates.length} documents in <code>{choosing.path}</code>.</p>
    {/if}
    {#each choosing.warnings as warning}
      <p class="quiet">{warning}</p>
    {/each}
    {#if chosenCandidate?.kind === 'markdown' && !choosing.toolchain.pandoc.available}
      <p class="bad" role="alert">
        pandoc was not found. Markdown is converted to LaTeX with pandoc before latexmk builds it.
      </p>
    {/if}
    {#if chosenCandidate && chosenCandidate.latexmkrcPaths.length > 0}
      <p class="bad" role="alert">
        There is executable latexmk configuration beside this document
        ({chosenCandidate.latexmkrcPaths.join(', ')}). It is Perl and it will run. Open this
        document only if you trust the folder.
      </p>
    {/if}
    <label>
      Document
      <select
        value={chosen}
        onchange={(event) => selectCandidate(event.currentTarget.value)}
      >
        {#each choosing.candidates as candidate}
          <option value={candidate.documentPath}>
            {candidate.name}{candidate.projectId === null ? '' : ' — already in Press'}
          </option>
        {/each}
      </select>
    </label>
    {#if chosenCandidate}
      <p class="quiet"><code>{chosenCandidate.documentPath}</code></p>
    {/if}
    <label>
      Engine
      <select bind:value={chosenEngine}>
        {#each ENGINES as engine}<option value={engine}>{engine}</option>{/each}
      </select>
    </label>
    <div class="dialog-actions">
      <button onclick={() => (choosing = null)} disabled={busy}>Cancel</button>
      <button onclick={confirmChoice} disabled={busy || !chosen}>Open and build</button>
    </div>
  </dialog>
{/if}

{#if snapshotOpen}
  <dialog open aria-labelledby="snapshot-title">
    <h2 id="snapshot-title">Snapshot this version</h2>
    <p class="quiet">
      Press stores the project's source as it is now, in its own history. Your project folder is
      not touched and no version control of yours is involved.
    </p>
    <label>
      Title
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:value={snapshotTitle}
        maxlength="72"
        autofocus
        placeholder="What changed?"
        onkeydown={(event) => {
          if (event.key === 'Enter') takeSnapshot();
          if (event.key === 'Escape') snapshotOpen = false;
        }}
      />
    </label>
    <details>
      <summary>Notes</summary>
      <textarea bind:value={snapshotBody} rows="3"></textarea>
    </details>
    <div class="dialog-actions">
      <button onclick={() => (snapshotOpen = false)} disabled={busy}>Cancel</button>
      <button onclick={takeSnapshot} disabled={busy || !snapshotTitle.trim()}>Snapshot</button>
    </div>
  </dialog>
{/if}

{#if renaming}
  <dialog open aria-labelledby="rename-title">
    <h2 id="rename-title">Rename this version</h2>
    <label>
      Title
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:value={renameTitle}
        maxlength="72"
        autofocus
        onkeydown={(event) => {
          if (event.key === 'Enter') saveRename();
          if (event.key === 'Escape') renaming = null;
        }}
      />
    </label>
    <div class="dialog-actions">
      <button onclick={() => (renaming = null)} disabled={busy}>Cancel</button>
      <button onclick={saveRename} disabled={busy || !renameTitle.trim()}>Save</button>
    </div>
  </dialog>
{/if}

{#if settingsFor}
  <dialog open aria-labelledby="settings-title">
    <h2 id="settings-title">{settingsFor.name}</h2>
    <label>
      Name
      <input bind:value={settingsName} maxlength="80" />
    </label>
    <p class="quiet"><code>{settingsFor.documentPath}</code></p>
    <label>
      Engine
      <select bind:value={settingsEngine}>
        {#each ENGINES as engine}<option value={engine}>{engine}</option>{/each}
      </select>
    </label>
    <p class="quiet">
      Changing the engine discards every cached PDF for this project: versions built by different
      engines cannot be compared. To compile a different document, open that document — it is its
      own project.
    </p>
    <div class="dialog-actions">
      <button onclick={() => (settingsFor = null)} disabled={busy}>Cancel</button>
      <button onclick={saveSettings} disabled={busy}>Save</button>
    </div>
  </dialog>
{/if}

{#if confirmDelete}
  <dialog open aria-labelledby="delete-title">
    <h2 id="delete-title">Remove {confirmDelete.name}?</h2>
    <p>
      Press forgets this document and deletes the PDFs it built.
      <code>{confirmDelete.documentPath}</code> and everything beside it are untouched.
    </p>
    <div class="dialog-actions">
      <button onclick={() => (confirmDelete = null)} disabled={busy}>Cancel</button>
      <button class="danger" onclick={removeProject} disabled={busy}>Remove</button>
    </div>
  </dialog>
{/if}

<style>
  /* The reset, tokens, fonts and scrollbars all live in `src/app.css`. */

  h1,
  h2,
  p {
    margin-top: 0;
  }

  .quiet {
    color: var(--ink-3);
  }

  .bad {
    color: var(--danger);
  }

  .spacer {
    flex: 1;
  }

  /* A borderless text button, so the chrome reads as text rather than controls. */
  .link {
    padding: 0.15rem 0.35rem;
    border: 0;
    border-radius: var(--radius-chip);
    background: none;
    color: var(--ink-2);
    cursor: pointer;
  }

  .link:hover:not(:disabled) {
    background: var(--paper-2);
    color: var(--ink);
  }

  .link:disabled {
    color: var(--line-3);
    cursor: default;
  }

  .link.bad {
    color: var(--danger);
  }

  /* -- reader ---------------------------------------------------------- */

  .reader {
    display: grid;
    grid-template-rows: minmax(0, 1fr) auto auto auto;
    height: 100vh;
    position: relative;
    /* A fixed app shell: the PDF scrolls inside its own pane and the rows above
       and below are sized to fit, so nothing here should ever scroll the window
       itself. Without this one long message drags the whole layout sideways. */
    overflow: hidden;
  }

  /* Overlaid rather than laid out, so it costs no vertical space. Kept narrow:
     it only has to cover the traffic lights, and everywhere else stays
     scrollable. */
  .drag-zone {
    position: absolute;
    top: 0;
    left: 0;
    z-index: 5;
    width: 11rem;
    height: 2.25rem;
  }

  /* Only used by the library, where the heading needs to clear the lights. */
  .titlebar {
    height: 2.5rem;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
    padding: 0.25rem 0.5rem;
    font-size: var(--fs-card);
  }

  .bar.bottom {
    border-top: var(--bw) solid var(--line);
    background: var(--card-2);
    color: var(--ink-3);
  }

  .title {
    overflow: hidden;
    color: var(--ink);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta,
  /* The one part of the footer that gives way. Everything else is a control
     whose label stops meaning anything once it is clipped, so the version line
     absorbs the whole squeeze and ellipsises. */
  .build {
    min-width: 0;
    overflow: hidden;
    color: var(--ink-3);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .bar > button,
  .bar > .quiet {
    flex: none;
  }

  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: var(--radius-pill);
    background: var(--line-3);
  }

  .dot.good {
    background: var(--accent);
  }

  .dot.bad {
    background: var(--danger);
  }

  .dot.busy {
    background: var(--amber);
  }

  /* The history sits beside the document rather than over it, so a version can
     be picked while still reading. */
  .document {
    display: flex;
    position: relative;
    min-height: 0;
  }

  .stage {
    flex: 1;
    min-width: 0;
  }

  /* Deep enough to clear the traffic lights, which the panel opens underneath.
     Only the history needs it: everywhere else the lights sit over the PDF's
     own margin. */
  .history-head {
    padding: 2.25rem 0.6rem 0.4rem;
    border-bottom: var(--bw) solid var(--line);
  }

  /* Over the document rather than beside it. Laid out in the flex row the
     panel took a column of its own, which narrowed the viewer and pushed the
     footer wider than the window. */
  .history {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    z-index: 4;
    width: 15rem;
    overflow-y: auto;
    border-right: var(--bw) solid var(--line);
    /* Opaque, because there is now a page underneath it. */
    background: var(--card-2);
    box-shadow: var(--shadow-menu);
    font-size: var(--fs-card);
  }

  .version {
    display: flex;
    align-items: flex-start;
    border-bottom: var(--bw) solid var(--line);
  }

  .version.current {
    background: var(--paper-2);
  }

  .version-open {
    display: grid;
    gap: 0.15rem;
    flex: 1;
    min-width: 0;
    padding: 0.4rem 0.6rem;
    border: 0;
    background: none;
    text-align: left;
    cursor: pointer;
  }

  .version-open .body {
    white-space: pre-wrap;
  }

  dialog textarea {
    width: 100%;
  }

  /* The same surface as the viewer, shown when a version has no PDF yet — so it
     matches, rather than flashing a different colour when one is selected. */
  .empty {
    display: grid;
    height: 100%;
    place-items: center;
    background: var(--paper);
    color: var(--ink-3);
  }

  .panel {
    max-height: 14rem;
    overflow: auto;
    padding: 0.5rem 0.75rem;
    border-top: var(--bw) solid var(--line);
    background: var(--paper-2);
    font-size: var(--fs-card);
  }

  .panel pre {
    margin: 0;
    white-space: pre-wrap;
  }

  .diagnostics {
    margin: 0;
    padding-left: 1.1rem;
  }

  .diagnostics li.error {
    color: var(--danger);
  }

  .diagnostics li.warning {
    color: var(--amber);
  }

  .diagnostics code {
    margin-right: 0.4rem;
  }

  .keys {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.2rem 0.75rem;
    margin: 0;
  }

  .keys dd {
    margin: 0;
    color: var(--ink-2);
  }

  kbd {
    font-family: ui-monospace, monospace;
  }

  .strip {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0;
    padding: 0.35rem 0.5rem;
    border-top: var(--bw) solid var(--line);
    background: var(--paper-2);
    font-size: var(--fs-card);
  }

  .strip.bad {
    background: var(--danger-bg);
  }

  .strip span {
    flex: 1;
    /* Both matter: a flex item will not shrink below its content without
       min-width, and a filesystem path has no spaces to break at, so without
       these one notice widens the whole window. */
    min-width: 0;
    overflow-wrap: anywhere;
  }

  /* -- library --------------------------------------------------------- */

    .library {
      /* was: padding: 0 1.5rem 1.5rem; */
      padding: 0 0 var(--gutter);
      background: var(--paper);
    }

    .titlebar {
      height: 2.375rem;   /* was 2.5rem — 1c opens on 38px */
    }

  .library-header,
  .library-actions,
  .dialog-actions,
  .project-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .library-header {
    justify-content: space-between;
    /* At a narrow window the Add button drops below the title rather than
       squeezing it off the screen. */
    flex-wrap: wrap;
  }
/* ── 2. Header ───────────────────────────────────────────────────────
   1c's title is Spectral 34px over a mono uppercase eyebrow, and the
   header baseline sits on the shelf rather than floating. */

.library-header h1 {
  margin: 0;
  font-family: var(--font-display);
  font-size: var(--fs-title);
  font-weight: var(--fw-title);
  line-height: 1;
  letter-spacing: var(--tracking-title);
  color: var(--ink);
}

/* The subtitle is the eyebrow: uppercase, tracked, one tier down in ink. */
.library-header p {
  margin: 0;
  font-size: var(--fs-label);
  font-weight: var(--fw-label);
  line-height: 1;
  letter-spacing: var(--tracking-label);
  text-transform: uppercase;
  color: var(--ink-3);
}

.library-header {
  /* was: align-items: center — 1c hangs both off the same baseline */
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 1.5rem;
  flex-wrap: wrap;
  padding: 0 var(--gutter) 1.625rem;  /* was: no padding of its own */
}

.library-header > div {
  display: flex;
  flex-direction: column;
  /* Both lines set line-height 1, so this gap is the whole space between
     them: 4px read as a collision under a two-word title. */
  gap: 0.5rem;
}

/* The one solid control on the page: ink fill, paper text, no shadow.
   NEW rule — today it is a bare <button> on UA styling. */
.library-actions button {
  padding: 0.5625rem 0.9375rem;
  border: 0;
  border-radius: var(--radius);
  background: var(--accent);
  color: var(--on-brand);
  font-family: var(--font-sans);
  font-size: var(--fs-body);
  font-weight: 500;
  line-height: 1;
  box-shadow: var(--shadow-btn);
  cursor: pointer;
  transition: background var(--ease);
}

.library-actions button:hover:not(:disabled) {
  background: var(--accent-strong);
}



.pinned-row {
  /* was: grid, auto-fill 190px tracks, gap .75rem, margin-bottom 1.25rem */
  display: flex;
  gap: var(--shelf-gap);
  margin: 0;
  padding: var(--shelf-pad-y) var(--gutter) 1.875rem;
  border-top: var(--bw) solid var(--line);
  border-bottom: var(--bw) solid var(--line);
  background: var(--paper-2);
}

.pinned-card {
  /* was: border + --radius + overflow hidden — 1c has no card chrome */
  border: 0;
  border-radius: 0;
  overflow: visible;
  width: var(--shelf-thumb-w);
  flex: none;
}

.pinned-open {
  display: flex;               /* was: grid, gap .4rem, padding 0 0 .5rem */
  flex-direction: column;
  gap: 0.75rem;
  width: 100%;
  padding: 0;
  border: 0;
  background: none;
  text-align: left;
  cursor: pointer;
}

/* A card is a button, and dragging across one should not leave a smear of
   selected filename behind. */
.pinned-open,
.project-open {
  user-select: none;
  -webkit-user-select: none;
}

/* NEW — the border, radius and shadow that used to be on the card now
   live on the page image, which is what actually looks like paper. */
.pinned-thumb {
  display: block;
  width: var(--shelf-thumb-w);
  height: var(--shelf-thumb-h);
  overflow: hidden;
  border-radius: var(--radius);
  background: var(--card);
  box-shadow: var(--shadow-md);
  transition: box-shadow var(--ease);
}

.pinned-open:hover:not(:disabled) .pinned-thumb {
  box-shadow: var(--shadow-sm);
}

/* Centred, name over kind — the shelf has the room the grid row does not,
   and the two lines are the same recipe either way (see `.name` / `.kind`). */
.pinned-lines {
  display: grid;
  gap: 0.375rem;
  justify-items: center;
  width: 100%;
  min-width: 0;
  text-align: center;
}



.project-grid {
  /* was: auto-fill minmax(240px, 1fr), gap .5rem */
  display: grid;
  grid-template-columns: repeat(var(--grid-cols), minmax(0, 1fr));
  gap: var(--grid-gap-y) var(--grid-gap-x);
  padding: 1.875rem var(--gutter) 0;
}

.project-card {
  display: flex;
  align-items: center;
  gap: 0.8125rem;
  padding: var(--row-pad);
  margin: calc(var(--row-pad) * -1);
  border: 0;
  border-radius: var(--radius-sm);
  overflow: visible;
  transition: background var(--ease), box-shadow var(--ease);
}

.project-card:hover {
  background: var(--card);          /* NEW */
  /* box-shadow: var(--shadow-plate); */
}

.project-open {
  /* The text block is shorter than the page image, so it is centred against
     it rather than hung from the top edge. */
  align-items: center;
  gap: 0.8125rem;            /* was: .5rem */
  display: flex;
  flex: 1;
  min-width: 0;
  padding: 0;
  border: 0;
  background: none;
  text-align: left;
  cursor: pointer;
}

.project-thumb {
  /* was: width 3.5rem, no chrome of its own */
  display: block;
  flex: none;
  width: var(--grid-thumb-w);
  height: var(--grid-thumb-h);
  overflow: hidden;
  /* border: var(--bw) solid var(--line-2); */
  border-radius: var(--radius-sm);
  background: var(--card);
  box-shadow: var(--shadow-sm);
  /* box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25); */
}

.project-lines {
  gap: 0.375rem;             /* was: .1rem */
  display: grid;
  flex: 1;
  min-width: 0;
}

/* When the PDF under the thumbnail was made, and how many versions are kept —
   the faintest tier, and the only line that changes on its own. */
.project-lines .when {
  overflow: hidden;
  font-size: var(--fs-meta);
  line-height: 1;
  color: var(--ink-3);
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* One recipe for both shelf and grid: a document's name and its kind read the
   same wherever it is shown. */
.kind {
  overflow: hidden;
  color: var(--ink-3);
  font: 600 var(--fs-label) var(--font-sans);
  letter-spacing: var(--tracking-label);
  text-transform: uppercase;
  /* A narrow column clips this line rather than pushing into its neighbour;
     the name above it is the part that has to survive. */
  text-overflow: ellipsis;
  white-space: nowrap;
}

.name {
    display: -webkit-box;
    overflow: hidden;
    font-size: var(--fs-card);
    font-weight: 600;
    line-height: 1.25;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  color: var(--ink);
  overflow-wrap: anywhere;
}

.missing-thumbnail {
  /* was: aspect-ratio 1/1.414 — the parent sets the box now */
  display: grid;
  width: 100%;
  height: 100%;
  place-items: center;
  background: var(--paper-3);
  color: var(--ink-4);
  font-size: var(--fs-label);
  text-align: center;
}


/* ── 7. Empty state ──────────────────────────────────────────────────
   Only needs the gutter it no longer inherits from .library. */

.empty-library {
  padding-inline: var(--gutter);
  display: grid;
  place-items: center;
  align-content: center;
  min-height: 55vh;
  text-align: center;
}

  /* -- context menu ----------------------------------------------------- */

  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
    padding: 0;
    border: 0;
    background: none;
    cursor: default;
  }

  .context-menu {
    position: fixed;
    z-index: 31;
    min-width: 11rem;
    margin: 0;
    padding: 0.25rem;
    list-style: none;
    background: var(--card);
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius);
    box-shadow: var(--shadow-menu);
    font-size: var(--fs-menu);
    user-select: none;
  }

  .context-menu button {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.3rem 0.5rem;
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--ink-2);
    text-align: left;
    white-space: nowrap;
    cursor: pointer;
  }

  /* The icon is the quieter half of the item until the row is under the
     pointer, when both halves move together. */
  .context-menu button :global(svg) {
    flex: none;
    color: var(--ink-3);
  }

  .context-menu button:hover:not(:disabled) {
    background: var(--paper-2);
    color: var(--ink);
  }

  .context-menu button:hover:not(:disabled) :global(svg) {
    color: var(--ink-2);
  }

  /* Destructive items sit below a rule and turn red whole — icon included —
     so the one item that cannot be undone never looks like the others. */
  .context-menu li.danger {
    margin-top: 0.25rem;
    padding-top: 0.25rem;
    border-top: var(--bw) solid var(--line);
  }

  .context-menu li.danger button:hover:not(:disabled) {
    background: var(--danger-bg);
    color: var(--danger);
  }

  .context-menu li.danger button:hover:not(:disabled) :global(svg) {
    color: var(--danger);
  }

  .context-menu button:disabled,
  .context-menu button:disabled :global(svg) {
    color: var(--line-3);
    cursor: default;
  }

  /* A non-modal <dialog open> defaults to its static position, which in the
     reader lands a full viewport below a 100vh grid — open and unreachable.
     Fixed and centred is what makes it visible at all.
     `right` and `margin` are reset with it: the user agent sets
     `inset-inline-end: 0` and `margin: auto` on every dialog, and left:50%
     against right:0 leaves the box centred in the right half of the window
     rather than in the window. */
  dialog {
    position: fixed;
    top: 50%;
    left: 50%;
    right: auto;
    bottom: auto;
    margin: 0;
    transform: translate(-50%, -50%);
    z-index: 20;
    width: min(30rem, calc(100% - 2rem));
    max-height: calc(100vh - 3rem);
    overflow-y: auto;
    padding: 1.25rem;
    background: var(--card);
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius);
    box-shadow: var(--shadow-popover);
    color: var(--ink);
    font-size: var(--fs-menu);
  }

  /* One step above the dialog's own text, and well below the library's
     title — a dialog announces itself without shouting. */
  dialog h2 {
    margin-bottom: 0.75rem;
    font-size: 0.9375rem;
    font-weight: 600;
    letter-spacing: var(--tracking-title);
  }

  dialog p {
    margin-bottom: 0.75rem;
    color: var(--ink-2);
    line-height: 1.45;
  }

  dialog p.quiet {
    color: var(--ink-3);
  }

  dialog code {
    font-size: var(--fs-meta);
    overflow-wrap: anywhere;
  }

  dialog label,
  dialog select,
  dialog input,
  dialog textarea {
    display: block;
    width: 100%;
  }

  /* The field name is the same eyebrow the library cards use. */
  dialog label {
    margin-bottom: 0.85rem;
    color: var(--ink-3);
    font-size: var(--fs-label);
    font-weight: var(--fw-label);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }

  dialog input,
  dialog select,
  dialog textarea {
    margin-top: 0.3rem;
    padding: 0.4rem 0.5rem;
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius-sm);
    background: var(--paper);
    color: var(--ink);
    font-size: var(--fs-menu);
    letter-spacing: normal;
    text-transform: none;
  }

  dialog summary {
    margin-bottom: 0.5rem;
    color: var(--ink-3);
    cursor: pointer;
  }

  dialog details {
    margin-bottom: 0.85rem;
  }

  .dialog-actions {
    justify-content: flex-end;
    gap: 0.5rem;
  }

  /* Cancel is an outline, the action beside it is the one filled control —
     the same pair the library header uses for its primary button. */
  .dialog-actions button {
    padding: 0.4rem 0.85rem;
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius);
    background: var(--card);
    color: var(--ink-2);
    font-size: var(--fs-menu);
    line-height: 1.2;
    cursor: pointer;
    transition: background var(--ease);
  }

  .dialog-actions button:hover:not(:disabled) {
    background: var(--paper-2);
    color: var(--ink);
  }

  .dialog-actions button:last-child {
    border-color: transparent;
    background: var(--accent);
    color: var(--on-brand);
  }

  .dialog-actions button:last-child:hover:not(:disabled) {
    background: var(--accent-strong);
    color: var(--on-brand);
  }

  /* Removing a document is the one confirm that undoes something. */
  .dialog-actions button.danger {
    background: var(--danger);
  }

  .dialog-actions button.danger:hover:not(:disabled) {
    background: var(--danger-strong);
  }

  .dialog-actions button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
