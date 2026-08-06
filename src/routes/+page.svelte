<script lang="ts">
  import { onMount } from 'svelte';
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
  type MenuItem = { label: string; run: () => void; disabled?: boolean };
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
      { label: 'Rename…', run: () => openSettings(project) },
      {
        label: 'Download PDF',
        run: () => void downloadArtifact(project.artifact?.id),
        disabled: !project.artifact
      },
      { label: 'Remove…', run: () => (confirmDelete = project) }
    ];
  }

  function versionMenu(version: VersionSummary): MenuItem[] {
    const items: MenuItem[] = [
      {
        label: 'Download PDF',
        run: () => void downloadArtifact(version.artifact?.id),
        disabled: !version.artifact
      }
    ];
    // The working tree is not a stored version, so it has neither of these.
    if (version.snapshot) {
      items.unshift({
        label: 'Rename…',
        run: () => {
          renaming = version;
          renameTitle = version.title;
        }
      });
      items.push({ label: 'Discard', run: () => void discardVersion(version) });
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

      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
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

  function lastBuilt(project: ProjectSummary) {
    const at = project.build.finishedAt ?? project.artifact?.builtAt;
    if (!at) return 'Never built';
    return new Date(at * 1000).toLocaleString();
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

{#if activeProject}
  <main class="reader">
    <!-- Not a bar: this takes no space in the layout. It sits over the grey
         gutter beside the traffic lights purely so the window can be dragged. -->
    <div class="drag-zone" data-tauri-drag-region></div>

    <section class="document">
      {#if showHistory}
        <nav class="history" aria-label="Version history">
          <div class="history-head quiet">{activeProject.name}</div>
          {#each versions as version (version.sourceRef)}
            <div class="version" class:current={version.sourceRef === selectedRef}>
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
              <button
                class="link"
                aria-label="Actions for {version.title}"
                onclick={(event) => openMenu(event, versionMenu(version))}
                disabled={busy}
              >
                ⋯
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
        <h1>Press</h1>
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
      <section class="project-grid" aria-label="Saved documents">
        {#each projects as project (project.id)}
          <article class="project-card">
            <button
              class="project-open"
              onclick={() => openProject(project)}
              oncontextmenu={(event) => openMenu(event, projectMenu(project))}
              disabled={busy || !project.available}
            >
              {#if project.artifact}
                {#key project.artifact.revision}
                  <PdfThumbnail artifact={project.artifact} />
                {/key}
              {:else}
                <div class="missing-thumbnail">No compiled PDF</div>
              {/if}
              <strong>{project.name}</strong>
              <span class="quiet">{project.kind} · {project.engine}</span>
              <span class="quiet">{lastBuilt(project)}</span>
              {#if !project.available}
                <span class="bad">Document missing</span>
              {/if}
            </button>
            <div class="project-actions">
              <button
                class="link"
                aria-label="Actions for {project.name}"
                onclick={(event) => openMenu(event, projectMenu(project))}
                disabled={busy}
              >
                ⋯
              </button>
            </div>
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
      <li>
        <button onclick={() => runMenuItem(item)} disabled={item.disabled}>{item.label}</button>
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
      <button onclick={removeProject} disabled={busy}>Remove</button>
    </div>
  </dialog>
{/if}

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(html),
  :global(body) {
    height: 100%;
    margin: 0;
    font-family: system-ui, sans-serif;
    font-size: 14px;
  }

  /* Scrollbars take no layout space.
     WebKit has no middle setting here: an unstyled scrollbar reserves 15px of
     gutter, and styling it to be thinner still reserves whatever width it is
     given — there is no CSS that produces the overlay kind. So they are hidden,
     and position is read from the page counter in the footer instead. */
  :global(::-webkit-scrollbar) {
    width: 0;
    height: 0;
  }

  :global(button),
  :global(select),
  :global(input) {
    font: inherit;
  }

  h1,
  h2,
  p {
    margin-top: 0;
  }

  .quiet {
    color: #767676;
  }

  .bad {
    color: #a4262c;
  }

  .spacer {
    flex: 1;
  }

  /* A borderless text button, so the chrome reads as text rather than controls. */
  .link {
    padding: 0.15rem 0.35rem;
    border: 0;
    border-radius: 3px;
    background: none;
    color: #444;
    cursor: pointer;
  }

  .link:hover:not(:disabled) {
    background: #0000000d;
    color: #000;
  }

  .link:disabled {
    color: #aaa;
    cursor: default;
  }

  .link.bad {
    color: #a4262c;
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
    font-size: 0.8125rem;
  }

  .bar.bottom {
    border-top: 1px solid #0000001a;
    background: #fafafa;
    color: #767676;
  }

  .title {
    overflow: hidden;
    color: #111;
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
    color: #767676;
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
    border-radius: 50%;
    background: #bbb;
  }

  .dot.good {
    background: #2d8a4e;
  }

  .dot.bad {
    background: #a4262c;
  }

  .dot.busy {
    background: #d18b1f;
  }

  /* The history sits beside the document rather than over it, so a version can
     be picked while still reading. */
  .document {
    display: flex;
    min-height: 0;
  }

  .stage {
    flex: 1;
    min-width: 0;
  }

  .history-head {
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid #0000001a;
  }

  .history {
    width: 15rem;
    overflow-y: auto;
    border-right: 1px solid #0000001a;
    font-size: 0.8125rem;
  }

  .version {
    display: flex;
    align-items: flex-start;
    border-bottom: 1px solid #0000000d;
  }

  .version.current {
    background: #0000000d;
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

  .empty {
    display: grid;
    height: 100%;
    place-items: center;
    background: #6f6f6f;
    color: #eee;
  }

  .panel {
    max-height: 14rem;
    overflow: auto;
    padding: 0.5rem 0.75rem;
    border-top: 1px solid #0000001a;
    background: #f4f4f4;
    font-size: 0.8125rem;
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
    color: #a4262c;
  }

  .diagnostics li.warning {
    color: #7a6320;
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
    color: #555;
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
    border-top: 1px solid #0000001a;
    background: #f4f4f4;
    font-size: 0.8125rem;
  }

  .strip.bad {
    background: #fdf2f2;
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
    padding: 0 1.5rem 1.5rem;
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

  .project-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 1rem;
  }

  .project-card {
    display: grid;
    border: 1px solid #0000001a;
    border-radius: 4px;
    overflow: hidden;
  }

  .project-open {
    display: grid;
    gap: 0.3rem;
    padding: 0;
    border: 0;
    background: none;
    text-align: left;
    cursor: pointer;
    /* A document name is a path fragment with no spaces to break at, so without
       this a long one widens its card and the whole grid with it. */
    overflow-wrap: anywhere;
  }

  .project-open > :global(strong),
  .project-open > :global(span) {
    margin-inline: 0.75rem;
  }

  .project-open > :global(span:last-child) {
    margin-bottom: 0.5rem;
  }

  .project-actions {
    gap: 0.25rem;
    padding: 0.25rem 0.5rem;
    border-top: 1px solid #0000001a;
  }

  .missing-thumbnail {
    display: grid;
    min-height: 180px;
    place-items: center;
    background: #ededed;
    color: #767676;
  }

  .empty-library {
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
    margin: 0;
    padding: 0.2rem 0;
    list-style: none;
    background: #fff;
    border: 1px solid #0000002a;
  }

  .context-menu button {
    display: block;
    width: 100%;
    padding: 0.25rem 1rem;
    border: 0;
    background: none;
    text-align: left;
    white-space: nowrap;
    cursor: pointer;
  }

  .context-menu button:hover:not(:disabled) {
    background: #0000000d;
  }

  .context-menu button:disabled {
    color: #aaa;
    cursor: default;
  }

  /* A non-modal <dialog open> defaults to its static position, which in the
     reader lands a full viewport below a 100vh grid — open and unreachable.
     Fixed and centred is what makes it visible at all. */
  dialog {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 20;
    width: min(34rem, calc(100% - 2rem));
    max-height: calc(100vh - 2rem);
    overflow-y: auto;
    border: 1px solid #0000002a;
    border-radius: 6px;
  }

  dialog label,
  dialog select,
  dialog input {
    display: block;
    width: 100%;
  }

  dialog label {
    margin-bottom: 0.6rem;
  }

  .dialog-actions {
    justify-content: flex-end;
  }
</style>
