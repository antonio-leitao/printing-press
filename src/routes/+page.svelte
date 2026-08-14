<script lang="ts">
  import { onMount, untrack, type Component } from 'svelte';
  import { fade } from 'svelte/transition';
  import { Download, Moon, Pencil, Pin, PinOff, Settings, Sun, Trash2 } from '@lucide/svelte';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { open } from '@tauri-apps/plugin-dialog';
  import PdfThumbnail from '$lib/PdfThumbnail.svelte';
  import PdfViewer, { type PeekRequest } from '$lib/PdfViewer.svelte';
  import { api, errorMessage } from '$lib/api';
  import { fail, notify } from '$lib/messages.svelte';
  import { modal } from '$lib/modal';
  import { theme } from '$lib/theme.svelte';
  import {
    ENGINES,
    WORKTREE,
    type BuildProgress,
    type BuildUpdate,
    type Engine,
    type OpenCandidate,
    type OpenRequest,
    type ArtifactSummary,
    type EditorCommand,
    type LooseDocument,
    type ProjectSummary,
    type SourcePeek,
    type VersionSummary,
    type WatcherError
  } from '$lib/types';

  /** Matches the `.pinned-card` track width below, so nothing larger is drawn. */
  const PINNED_THUMB_WIDTH = 150
  /** Likewise for `.project-thumb`. */
  const GRID_THUMB_WIDTH = 64

  const KEY_HELP: Array<[string, string]> = [
    ['j / k', 'scroll down / up'],
    ['h / l', 'scroll left / right'],
    ['d / u', 'half page down / up'],
    ['f / b / space', 'page down / up'],
    ['J / K', 'next / previous page'],
    ['gg / G', 'first / last page'],
    ['12G', 'go to page 12'],
    ['⌃o / ⌃i', 'back / forward after a jump'],
    ['+ / -', 'zoom in / out'],
    ['0', 'actual size'],
    ['a / s', 'fit page / fit width'],
    ['⌃r', 'the dark theme — the page, and Press around it'],
    ['click', 'follow a reference, a citation or a link'],
    ['⌘click', 'the source behind a place in the PDF'],
    ['R', 'rebuild this version'],
    ['⌘K', 'snapshot the working tree'],
    ['?', 'this list']
  ];

  let projects = $state<ProjectSummary[]>([]);
  let activeProject = $state<ProjectSummary | null>(null);
  /**
   * A PDF Press is showing but does not own. Never both this and a project: the
   * reader shows one document, and this one has no history, no builds and no
   * source to snapshot.
   */
  let viewing = $state<LooseDocument | null>(null);
  /** Documents a path resolved to, shown when there is a choice to make. */
  let choosing = $state<OpenRequest | null>(null);
  let chosen = $state('');
  let chosenEngine = $state<Engine>('pdflatex');
  let busy = $state(false);
  /**
   * Whether Press still has to find out what it was started for. Nothing is
   * shown until it knows: a path from the command line or the editor goes
   * straight to the reader, and putting the library up in the meantime shows
   * the wrong screen for as long as the asking takes.
   */
  let opening = $state(true);
  /** How long Press will wait on a document before showing the library anyway. */
  const OPEN_GRACE = 4000;
  let buildLog = $state('');
  let progress = $state<BuildProgress | null>(null);
  let settingsFor = $state<ProjectSummary | null>(null);
  let settingsName = $state('');
  let settingsEngine = $state<Engine>('pdflatex');
  /// What the Editor button runs. A command line rather than a list of editors
  /// Press knows: an editor Press has never heard of is one line here, and
  /// anything that will not fit on one line fits in a script, which is a better
  /// home for it than a table inside Press.
  let editorOpen = $state(false);
  let editorCommand = $state('');
  let editorDefault = $state<EditorCommand | null>(null);
  let confirmDelete = $state<ProjectSummary | null>(null);
  let confirmDiscard = $state<VersionSummary | null>(null);

  // -- history -------------------------------------------------------------
  let versions = $state<VersionSummary[]>([]);
  /** Which row of the history the viewer is showing. */
  let selectedKey = $state<string>(WORKTREE);
  let showHistory = $state(false);
  let snapshotOpen = $state(false);
  let snapshotTitle = $state('');
  let snapshotBody = $state('');
  let renaming = $state<VersionSummary | null>(null);
  let renameTitle = $state('');

  /// What identifies a row of the history. Not the source ref: a ref carries a
  /// revision, and a revision is a content hash, so any two snapshots of the
  /// same source answer to the same ref. Press no longer stores a second one —
  /// see `create_snapshot` — but databases written before that rule have
  /// pairs in them, and a duplicate key takes the whole panel down.
  function versionKey(version: VersionSummary) {
    return version.snapshot ? `snapshot-${version.snapshot.id}` : WORKTREE;
  }

  const selected = $derived(
    versions.find((version) => versionKey(version) === selectedKey) ?? versions[0] ?? null
  );
  /** What the viewer builds and shows — shared by versions that hold the same source. */
  const selectedRef = $derived(selected?.sourceRef ?? WORKTREE);
  /** The working tree's artifact is the fallback while a version is still building. */
  const shownArtifact = $derived<ArtifactSummary | null>(
    viewing
      ? {
          // A loose PDF has no build behind it, but the viewer asks a document
          // for only two things: which id to fetch pages from, and which
          // revision, so that a file rewritten on disk is drawn again.
          id: viewing.id,
          projectId: -1,
          sourceRef: WORKTREE,
          engine: 'pdflatex',
          pageCount: null,
          byteSize: 0,
          builtAt: 0,
          revision: viewing.revision
        }
      : (selected?.artifact ?? null)
  );

  /** Whether the reader is on screen at all, whoever owns what it is showing. */
  const reading = $derived(activeProject !== null || viewing !== null);

  /// A dark library shows its documents the way a dark reader would: the page a
  /// thumbnail is of is drawn inverted, so the shelf holds the same pages you
  /// would be reading rather than a row of white rectangles.
  const dark = $derived(theme.isDark);

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
    try {
      await api.setProjectPinned(project.id, pinned);
      // Refetched rather than merged: pinning changes the order, and the
      // backend is what decides the order.
      await refreshProjects();
    } catch (reason) {
      fail(reason);
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
        label: 'Discard…',
        icon: Trash2,
        danger: true,
        run: () => (confirmDiscard = version)
      });
    }
    return items;
  }

  async function downloadArtifact(artifactId: number | undefined) {
    if (artifactId === undefined) return;
    try {
      notify(`Saved ${await api.exportArtifact(artifactId)}`);
    } catch (reason) {
      fail(reason);
    }
  }

  const chosenCandidate = $derived(
    choosing?.candidates.find((candidate) => candidate.documentPath === chosen) ?? null
  );

  // Two sections, one list: the backend already returns pinned first, so
  // splitting it here keeps both in the order it decided.
  const pinned = $derived(projects.filter((project) => project.pinned));
  const unpinned = $derived(projects.filter((project) => !project.pinned));

  /// What the library holds, by what it is written in. This line used to be a
  /// sentence about what Press is — read once on the first run and then read
  /// past for good, which is a poor use of the only line under the title. The
  /// sentence still exists, on the empty screen, where it is the answer to a
  /// question somebody actually has.
  ///
  /// A kind with none of it says nothing rather than saying zero.
  const librarySummary = $derived.by(() => {
    const names: Record<string, string> = { latex: 'LaTeX', markdown: 'Markdown' };
    const counts = new Map<string, number>();
    for (const project of projects) counts.set(project.kind, (counts.get(project.kind) ?? 0) + 1);
    return [...counts]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([kind, count]) => `${count} ${names[kind] ?? kind}`)
      .join(' · ');
  });

  const dialogOpen = $derived(
    Boolean(
      choosing ||
        settingsFor ||
        editorOpen ||
        confirmDelete ||
        confirmDiscard ||
        snapshotOpen ||
        renaming ||
        menu
    )
  );

  onMount(() => {
    const unlisteners: Array<() => void> = [];
    let disposed = false;

    void (async () => {
      try {
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
          // The file Press is showing was rewritten by whatever builds it. A new
          // revision is all the viewer needs to draw it again.
          await listen<{ id: number; revision: number }>('viewing-changed', (event) => {
            if (viewing?.id !== event.payload.id) return;
            viewing = { ...viewing, revision: event.payload.revision };
          })
        );
        unlisteners.push(
          await listen<WatcherError>('watcher-error', (event) => {
            // Not a build failure: the document is fine, Press just cannot see
            // saves. A warning, because nothing is broken but something is lost.
            notify(event.payload.message, 'warning');
          })
        );
        unlisteners.push(
          // Only a nudge: the request itself is collected, so a missed event or a
          // webview that was not listening yet costs nothing.
          await listen('open-requested', () => {
            void collectPendingOpen().finally(() => (opening = false));
          })
        );
        unlisteners.push(
          // Tauri takes the drop itself rather than letting the webview see it,
          // so this is the only way to hear about one. Only the library answers:
          // pulling the document out from under someone mid-read because a file
          // passed over the window is not something they asked for.
          await getCurrentWebview().onDragDropEvent((event) => {
            const drag = event.payload;
            if (drag.type === 'enter' || drag.type === 'over') {
              dropping = !reading;
            } else if (drag.type === 'leave') {
              dropping = false;
            } else if (drag.type === 'drop') {
              const wanted = dropping;
              dropping = false;
              if (wanted) void acceptDrop(drag.paths);
            }
          })
        );
      } catch (reason) {
        // Press without live updates is a Press that still opens documents and
        // still builds them, only without saying so as it goes. Press stuck
        // behind the gate below is a blank window with nothing to click, so a
        // channel that will not open must not take the interface with it.
        fail(reason);
      }
      if (!disposed) {
        // Before the project list, so a database that was set aside explains
        // why the list is empty.
        try {
          const startup = await api.takeStartupNotice();
          if (startup) notify(startup);
        } catch {
          // A missing notice is not worth reporting.
        }
        try {
          // Ahead of the library, because this is the question that decides
          // which screen there is going to be. The listeners are registered
          // first and only first: a build started here would otherwise finish
          // before anything was listening for it.
          await collectPendingOpen();
          // Nothing was waiting, but something may still be coming: a path
          // from the command line is resolved in the background, and that
          // takes long enough to finish after the interface has started.
          const coming = !reading && (await api.expectingOpen().catch(() => false));
          await refreshProjects();
          // The request will arrive on its own channel and drop the gate.
          // Until then, and for no longer than this, nothing is shown.
          if (coming) window.setTimeout(() => (opening = false), OPEN_GRACE);
          else opening = false;
        } catch {
          opening = false;
        }
      }
    })();

    const shortcuts = (event: KeyboardEvent) => {
      // Before the guards below, which treat an open menu as a dialog.
      if (event.key === 'Escape' && (menu || peek)) {
        menu = null;
        peek = null;
        return;
      }
      if (dialogOpen) return;
      const target = event.target;
      const typing =
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target instanceof HTMLElement && target.isContentEditable);
      if (typing) return;

      // The library's own keys. ⌃r is not a second theme key beside the
      // viewer's — it is the same one, answered here because the viewer that
      // usually answers it is only mounted while reading. Kept to the two that
      // need no selection: what a key would act on is still being worked out.
      if (!reading) {
        const isR = event.code === 'KeyR' || event.key.toLowerCase() === 'r';
        if (event.ctrlKey && !event.metaKey && !event.altKey && isR) {
          event.preventDefault();
          theme.toggle();
          return;
        }
        const isO = event.code === 'KeyO' || event.key.toLowerCase() === 'o';
        if ((event.metaKey || event.ctrlKey) && !event.altKey && isO) {
          event.preventDefault();
          void chooseDocument();
        }
        return;
      }

      if (!activeProject) return;

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

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
      window.removeEventListener('keydown', shortcuts);
      clearTimeout(barHide);
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
      fail(reason);
    }
  }

  /** A document is over the window and the library will take it if let go. */
  let dropping = $state(false);

  /// A document dropped on the library. The same road in as the picker and as
  /// `:Press`: resolve the path, then let `present` decide whether it can just
  /// be opened or something has to be asked first. Nothing is special-cased
  /// here, which is what keeps the `.latexmkrc` consent on the way in.
  ///
  /// One document. Press shows one at a time, and adding the rest quietly in
  /// the background would be adding documents nobody watched being added.
  async function acceptDrop(paths: string[]) {
    const [first, ...rest] = paths;
    if (!first) return;
    busy = true;
    try {
      await present(await api.resolvePath(first));
      if (rest.length > 0) {
        notify(`Press opens one document at a time. ${rest.length} other ${
          rest.length === 1 ? 'file was' : 'files were'
        } left alone.`);
      }
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
    }
  }

  /// Point at the document. A folder is never a project, so there is nothing to
  /// pick a folder for: `:Press` and `press <path>` can still hand over a
  /// directory, and it opens the same picker this does.
  async function chooseDocument() {
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        title: 'Open a document',
        filters: [
          {
            name: 'Documents',
            // PDFs among them: Press shows one without taking it in.
            extensions: ['tex', 'ltx', 'Rnw', 'md', 'markdown', 'qmd', 'mkd', 'pdf']
          }
        ]
      });
      if (!selected || Array.isArray(selected)) return;
      busy = true;
      await present(await api.resolvePath(selected));
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
    }
  }

  /// Acts on what a path resolved to. One candidate and nothing to warn about
  /// opens it; anything else asks. Shared by every way into Press.
  async function present(request: OpenRequest) {
    // Before the toolchain check: showing a PDF needs no TeX installed, and a
    // machine without latexmk can still be a machine that reads papers.
    if (request.pdf) {
      await viewPdf(request.pdf);
      return;
    }
    if (!request.toolchain.latexmk.available) {
      notify('latexmk was not found. Install a TeX distribution or add latexmk to PATH.', 'error');
      return;
    }
    if (request.candidates.length === 0) {
      notify(request.warnings[0] ?? `Press found no document in ${request.path}.`, 'error');
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
      busy = true;
      try {
        await present(request);
      } finally {
        busy = false;
      }
    } catch (reason) {
      fail(reason);
    }
  }

  async function confirmChoice() {
    if (!chosenCandidate) return;
    busy = true;
    try {
      await openCandidate(chosenCandidate, chosenEngine);
    } catch (reason) {
      fail(reason);
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

  /// Opens a PDF for reading only. It stays out of the library, and Press
  /// watches it so that whatever is rebuilding it — a Makefile, `latexmk -pvc`
  /// — puts a fresh page on screen without being asked.
  async function viewPdf(path: string) {
    busy = true;
    try {
      // Opened before anything is let go of, so the reader never blinks back
      // to the library in between two documents.
      const document = await api.openPdf(path);
      if (activeProject) await api.closeProject();
      activeProject = null;
      viewing = document;
      panel = 'none';
      showHistory = false;
      versions = [];
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
    }
  }

  async function openProject(project: ProjectSummary) {
    busy = true;
    try {
      const opened = await api.openProject(project.id);
      await closeViewing();
      activeProject = opened;
      panel = 'none';
      selectedKey = WORKTREE;
      await refreshVersions();
    } catch (reason) {
      fail(reason);
      activeProject = null;
    } finally {
      busy = false;
    }
  }

  /// Lets go of a PDF Press was only showing, which also stops watching it.
  async function closeViewing() {
    const open = viewing;
    viewing = null;
    if (open) await api.closePdf(open.id);
  }

  async function returnToLibrary() {
    busy = true;
    try {
      await closeViewing();
      await api.closeProject();
      activeProject = null;
      buildLog = '';
      progress = null;
      panel = 'none';
      versions = [];
      showHistory = false;
      await refreshProjects();
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
    }
  }

  async function refreshVersions() {
    if (!activeProject) return;
    try {
      versions = await api.listVersions(activeProject.id);
      if (!versions.some((version) => versionKey(version) === selectedKey)) {
        selectedKey = WORKTREE;
      }
    } catch (reason) {
      fail(reason);
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
    try {
      const outcome = await api.createSnapshot(
        activeProject.id,
        snapshotTitle.trim(),
        snapshotBody.trim() || undefined
      );
      snapshotOpen = false;
      // Nothing was stored, because there was nothing new to store. Not a
      // failure: the source is kept already, under the name this carries.
      if (outcome.status === 'unchanged') {
        notify(`Nothing has changed since “${outcome.title}”.`);
        return;
      }
      await refreshVersions();
      // Show what was just stored; it is already building.
      selectedKey = `snapshot-${outcome.id}`;
      showHistory = true;
      notify(`Stored “${outcome.title}” — ${outcome.fileCount} files.`);
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
    }
  }

  /** Shows a version, building it first if it has never been compiled. */
  async function selectVersion(version: VersionSummary) {
    selectedKey = versionKey(version);
    progress = null;
    buildLog = '';
    // Open on the log and switching versions: read the one now being shown,
    // rather than leaving the panel empty until it is closed and reopened.
    if (panel === 'log') void loadBuildLog();
    if (version.artifact || !activeProject) return;
    if (version.build.status === 'running' || version.build.status === 'queued') return;
    try {
      await api.buildProject(activeProject.id, version.sourceRef);
    } catch (reason) {
      fail(reason);
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
      fail(reason);
    } finally {
      busy = false;
    }
  }

  async function discardVersion() {
    const version = confirmDiscard;
    if (!version?.snapshot) return;
    busy = true;
    try {
      await api.deleteSnapshot(version.snapshot.id);
      if (selectedKey === versionKey(version)) selectedKey = WORKTREE;
      confirmDiscard = null;
      await refreshVersions();
      notify(`Discarded “${version.title}”. The document itself was not touched.`);
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
    }
  }

  /// A card's last line. Nothing is said twice: when a document is in order
  /// this is how long ago it built and how much history is behind it, and when
  /// it is not, that takes the line instead and takes a colour with it.
  ///
  /// Quiet is the healthy state on purpose. A library where every card reports
  /// the same thing is a library spending its reader's attention on nothing;
  /// what earns a colour here is the handful that need doing something about.
  function cardState(project: ProjectSummary): { text: string; tone: string } {
    if (!project.available) return { text: 'missing', tone: 'bad' };
    const { status } = project.build;
    if (status === 'queued' || status === 'running') return { text: 'building…', tone: 'busy' };
    // An artifact that no longer compiles is still worth flagging: the
    // thumbnail beside this is showing a PDF that no longer matches the source.
    if (status === 'error') return { text: 'does not compile', tone: 'bad' };
    const built = age(project.artifact?.builtAt);
    if (!built) return { text: 'never built', tone: '' };
    // Snapshots, not versions: every document has a version — the working tree
    // is one — so a count of those would never be lower than one and would say
    // nothing. A count of snapshots is a count of the states it can be put back
    // into, which is why one of them is already worth saying.
    const kept = project.snapshotCount;
    return { text: kept === 0 ? built : `${built} · ${kept} snapshot${kept === 1 ? '' : 's'}`, tone: '' };
  }

  function age(seconds: number | null | undefined) {
    if (!seconds) return '';
    const elapsed = Math.max(0, Math.floor(Date.now() / 1000 - seconds));
    if (elapsed < 60) return 'just now';
    if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
    if (elapsed < 86400) return `${Math.floor(elapsed / 3600)}h ago`;
    return `${Math.floor(elapsed / 86400)}d ago`;
  }

  /// The history's timestamp: one number and one letter, wide enough for a
  /// glance and narrow enough to sit in a gutter beside the title. The units
  /// are the ones the rest of the world abbreviates this way — s, m, h, d, w,
  /// then `mo` for months, because `m` is already minutes, and y.
  function shortAge(seconds: number) {
    const elapsed = Math.max(0, Math.floor(Date.now() / 1000 - seconds));
    if (elapsed < 60) return `${elapsed}s`;
    if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m`;
    if (elapsed < 86400) return `${Math.floor(elapsed / 3600)}h`;
    if (elapsed < 604800) return `${Math.floor(elapsed / 86400)}d`;
    if (elapsed < 2592000) return `${Math.floor(elapsed / 604800)}w`;
    if (elapsed < 31536000) return `${Math.floor(elapsed / 2592000)}mo`;
    return `${Math.floor(elapsed / 31536000)}y`;
  }

  async function rebuild() {
    if (!activeProject) return;
    try {
      await api.buildProject(activeProject.id, selectedRef);
    } catch (reason) {
      fail(reason);
    }
  }

  /// Opens the document in whatever the reader writes with, and has nothing
  /// more to do with it. Press watches the working tree, so a save rebuilds the
  /// document whoever wrote it — there is no channel to hold open and no editor
  /// process to keep track of.
  async function launchEditor() {
    if (!activeProject) return;
    try {
      notify(await api.launchEditor(activeProject.id));
    } catch (reason) {
      fail(reason);
    }
  }

  // -- preferences -----------------------------------------------------------

  async function openEditorSettings() {
    try {
      const current = await api.editorCommand();
      editorDefault = current;
      // An unset command shows the default it is standing in for, so that
      // editing it is a change to something visible rather than to a blank.
      editorCommand = current.command;
      editorOpen = true;
    } catch (reason) {
      fail(reason);
    }
  }

  async function saveEditorCommand() {
    busy = true;
    try {
      await api.setEditorCommand(editorCommand.trim());
      editorOpen = false;
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
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
    try {
      const name = settingsName.trim();
      if (name && name !== project.name) {
        mergeProject(await api.renameProject(project.id, name));
      }
      if (settingsEngine !== project.engine) {
        // Discards every cached PDF. The new build produces a new artifact id,
        // so nothing stale can be shown.
        mergeProject(await api.setProjectEngine(project.id, settingsEngine));
        notify('Engine changed. Cached PDFs were discarded and a rebuild has started.');
      }
      settingsFor = null;
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
    }
  }

  async function removeProject() {
    const project = confirmDelete;
    if (!project) return;
    busy = true;
    try {
      await api.deleteProject(project.id);
      projects = projects.filter((item) => item.id !== project.id);
      if (activeProject?.id === project.id) {
        activeProject = null;
        buildLog = '';
        progress = null;
      }
      confirmDelete = null;
      notify(`Removed ${project.name} from Press. The document itself was not touched.`);
    } catch (reason) {
      fail(reason);
    } finally {
      busy = false;
    }
  }

  /// The version being read, so the log belongs to the build whose errors are
  /// listed beside it. Each version keeps its own.
  async function loadBuildLog() {
    if (!activeProject) return;
    try {
      buildLog = await api.getBuildLog(activeProject.id, selectedRef);
    } catch (reason) {
      buildLog = errorMessage(reason);
    }
  }

  async function togglePanel(next: Panel) {
    panel = panel === next ? 'none' : next;
    if (panel === 'log' && !buildLog) await loadBuildLog();
  }

  // -- peek ------------------------------------------------------------------

  /// The source behind a place in the PDF, shown where it was asked for.
  ///
  /// Read-only on purpose. That is what makes it work on a stored version as
  /// well as the live one: the source comes from whatever this PDF was built
  /// from, and for a snapshot that is the object store rather than the folder,
  /// which may have moved on years ago.
  type Peek = {
    x: number;
    y: number;
    /** The viewer's own rectangle: the walls this has to stay inside. */
    bounds: DOMRect;
    version: string;
    source: SourcePeek | null;
    /** Set once the answer is in and there is nothing to show. */
    empty: boolean;
  };
  let peek = $state<Peek | null>(null);
  let peekElement = $state<HTMLElement | null>(null);
  let peekRequest = 0;

  /** Clear of the pointer, and clear of the wall. */
  const PEEK_GAP = 12;
  const PEEK_EDGE = 10;

  /// Puts the popover beside the click without letting it out of the viewer.
  ///
  /// Measured rather than guessed: how tall it is depends on how much source
  /// came back, which is not known until it is on screen. It opens downwards
  /// when there is room, flips up when there is not, and when neither side can
  /// hold it whole it takes the roomier one and scrolls inside itself.
  function placePeek(element: HTMLElement, at: Peek) {
    const walls = at.bounds;
    const below = walls.bottom - PEEK_EDGE - (at.y + PEEK_GAP);
    const above = at.y - PEEK_GAP - (walls.top + PEEK_EDGE);

    // Capped before measuring, so what is measured is what will be shown.
    element.style.maxHeight = `${Math.max(Math.max(below, above), 80)}px`;
    element.style.maxWidth = `${Math.min(512, walls.width - PEEK_EDGE * 2)}px`;
    const box = element.getBoundingClientRect();

    const room = (space: number) => box.height <= space;
    const top = room(below) || below >= above ? at.y + PEEK_GAP : at.y - PEEK_GAP - box.height;
    const left = at.x + PEEK_GAP;

    element.style.left = `${clamp(left, walls.left + PEEK_EDGE, walls.right - PEEK_EDGE - box.width)}px`;
    element.style.top = `${clamp(top, walls.top + PEEK_EDGE, walls.bottom - PEEK_EDGE - box.height)}px`;
    element.style.visibility = 'visible';
  }

  /// Low wins when the two cross, which is what happens when the popover is
  /// wider or taller than the space it has to sit in.
  function clamp(value: number, low: number, high: number) {
    return Math.max(low, Math.min(value, Math.max(low, high)));
  }

  // Placed after every change of content: the answer arrives after the popover
  // is already on screen, and it is a different size when it does.
  $effect(() => {
    const element = peekElement;
    const at = peek;
    if (!element || !at) return;
    void at.source;
    void at.empty;
    placePeek(element, at);
  });

  async function peekSource(at: PeekRequest) {
    const artifact = shownArtifact;
    if (!artifact) return;
    const request = ++peekRequest;
    peek = {
      x: at.clientX,
      y: at.clientY,
      bounds: at.bounds,
      version: viewing ? viewing.name : (selected?.title ?? 'Working tree'),
      source: null,
      empty: false
    };
    try {
      const source = await api.peekSource(artifact.id, at.page, at.x, at.y);
      // A second click while the first was in flight wins.
      if (request !== peekRequest || !peek) return;
      peek = { ...peek, source, empty: source === null };
    } catch (reason) {
      if (request === peekRequest) peek = null;
      fail(reason);
    }
  }

  async function copyPeek() {
    const text = peek?.source?.text;
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      peek = null;
      notify('Copied.');
    } catch (reason) {
      fail(reason);
    }
  }

  // -- build progress --------------------------------------------------------

  /** The stage a markdown build starts in, named the same way in `runner.rs`. */
  const PANDOC_STAGE = 'pandoc';

  /// Where the bar stands at the end of each pdflatex pass. How many passes a
  /// build needs is not knowable while it runs — latexmk reruns TeX until the
  /// cross-references stop moving — so each pass covers a shorter stretch than
  /// the one before it. The bar closes on the end without ever arriving, and
  /// the build finishing is what fills it.
  const PASS_MARKS = [0.06, 0.55, 0.78, 0.9, 0.96];

  /// A guess at how far along a build is, in 0..1. Every input is something
  /// latexmk actually said, so this is an estimate but never a fiction: the
  /// pass tells us which stretch of the bar we are in, and the page within
  /// that pass tells us how far across it.
  function buildFraction() {
    if (!progress) return PASS_MARKS[0];
    if (progress.stage === PANDOC_STAGE) return 0.04;

    // latexmk names the rule before TeX says which run it is, and says nothing
    // at all until it starts. Neither is a finished pass.
    const pass = progress.pass ?? 0;
    if (pass === 0) return PASS_MARKS[0];

    const started = PASS_MARKS[Math.min(pass - 1, PASS_MARKS.length - 1)];
    const finished = PASS_MARKS[Math.min(pass, PASS_MARKS.length - 1)];
    // biber, makeindex and friends ship no pages. They run between passes, so
    // the pass that just ended is as far as we can honestly claim to be.
    // Matched by prefix: `bibtex main` ends in tex without being an engine.
    if (!ENGINES.some((engine) => progress?.stage.startsWith(engine))) return finished;
    if (!progress.page) return started;

    const expected = progress.expectedPages ?? 0;
    // Without a page count from a previous build there is no denominator, so
    // the pages approach the end of the stretch instead of dividing it.
    const across =
      expected > 0
        ? Math.min(progress.page / expected, 1)
        : 1 - Math.exp(-progress.page / 12);
    return started + (finished - started) * across;
  }

  /// How full the bar is drawn. Held at whatever it reached, so a stage that
  /// knows less than the one before it cannot walk the bar backwards.
  let barValue = $state(0);
  let barShown = $state(false);
  let barBuilding = false;
  /// Held outside the effect on purpose. As the effect's own cleanup it would
  /// be cancelled by the next build update, and the finished bar would stay on
  /// screen for good.
  let barHide: ReturnType<typeof setTimeout> | undefined;

  $effect(() => {
    const status = selected?.build.status;
    const building = status === 'running' || status === 'queued';
    const next = building ? buildFraction() : 0;

    untrack(() => {
      if (building) {
        clearTimeout(barHide);
        // A new build starts the bar over rather than resuming the last one's.
        if (!barBuilding) barValue = 0;
        barBuilding = true;
        barShown = true;
        barValue = Math.max(barValue, status === 'queued' ? 0.02 : next);
        return;
      }
      // Nothing is building. Either the bar is already on its way out — leave
      // that timer alone — or this update is the one that ended the build.
      if (!barBuilding) return;
      barBuilding = false;
      // Filled before it goes: a bar that vanishes at two thirds reads as a
      // build that gave up.
      barValue = 1;
      barHide = setTimeout(() => (barShown = false), 260);
    });
  });

  /// What the footer says: which version, and whether it is current.
  function versionLabel() {
    // A PDF Press only shows has no version and no build to report. Its name
    // and the fact that Press is watching it is the whole story.
    if (viewing) return `${viewing.name} · watching for changes`;
    const version = selected;
    const name = version && version.sourceRef !== WORKTREE ? version.title : 'Working tree';
    const build = version?.build ?? activeProject?.build;
    if (!build) return name;

    if (build.status === 'queued') return `${name} · queued`;
    if (build.status === 'running') {
      if (progress?.stage === PANDOC_STAGE) return `${name} · converting markdown`;
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

  // From the version being read, not from the project. The project's own build
  // state is the working tree's, so selecting a snapshot that fails used to
  // report "does not compile" over the working tree's errors — two different
  // builds, side by side, reading as one.
  const diagnostics = $derived(
    (selected?.build ?? activeProject?.build)?.diagnostics ?? []
  );
  const errors = $derived(diagnostics.filter((item) => item.severity === 'error'));
  const warnings = $derived(diagnostics.filter((item) => item.severity === 'warning'));
  // The viewer keeps its own load error, because it also decides what the pane
  // shows when a document will not open. This only repeats it as a message, so
  // a failure is not silent when the pane still has the previous document on
  // screen.
  $effect(() => {
    if (viewerError) notify(viewerError, 'error');
  });
</script>

{#if reading}
  <main class="reader">
    <!-- Not a bar: this takes no space in the layout. It sits over the grey
         gutter beside the traffic lights purely so the window can be dragged. -->
    <div class="drag-zone" data-tauri-drag-region></div>

    <section class="document">
      {#if showHistory && activeProject}
        <nav class="history" aria-label="Version history">
          <!-- Clears the traffic lights, which sit over this panel's top left
               whenever it is open. -->
          <div class="history-head quiet">{activeProject.name}</div>
          {#each versions as version (versionKey(version))}
            <div class="version" class:current={versionKey(version) === selectedKey}>
              <!-- Right-click for rename, download and discard. -->
              <button
                class="version-open"
                onclick={() => selectVersion(version)}
                oncontextmenu={(event) => openMenu(event, versionMenu(version))}
              >
                <span class="version-head">
                  <strong>{version.title}</strong>
                  {#if version.snapshot}
                    <span class="stamp quiet">{shortAge(version.snapshot.createdAt)}</span>
                  {/if}
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
            onPeek={peekSource}
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

    <footer class="bar bottom">
      {#if barShown}
        <!-- Drawn over the footer's own hairline, so a build adds no height. -->
        <span
          class="progress"
          style="--filled: {barValue}"
          role="progressbar"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={Math.round(barValue * 100)}
          out:fade={{ duration: 200 }}
        ></span>
      {/if}
      <!-- Everything a project has and a PDF Press only shows does not:
           a history to open, builds to fail, a source to snapshot, an editor to
           open it in. What is left is what reading needs. -->
      {#if activeProject}
        <button class="link" onclick={() => (showHistory = !showHistory)} title="Version history">
          {showHistory ? '◀' : '☰'}
        </button>
      {/if}
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
      {#if activeProject}
        <!-- ⌘K as well, but a keystroke nobody can see is not a way to reach a
             feature, and it depends on the webview getting the key at all. -->
        <button class="link" onclick={openSnapshotDialog} disabled={busy} title="Snapshot (⌘K)">
          Snapshot
        </button>
        <button class="link" onclick={launchEditor} disabled={busy}>Editor</button>
      {/if}
      <button class="link" onclick={returnToLibrary} disabled={busy}>Projects</button>
      <button class="link" onclick={() => togglePanel('keys')} title="Keys (?)">?</button>
    </footer>
  </main>
{:else if opening}
  <!-- Press is still finding out what it was started for. Bare on purpose: a
       spinner for something this short is more noticeable than the wait. -->
  <main class="starting" data-tauri-drag-region></main>
{:else}
  <main class="library">
    <div class="titlebar" data-tauri-drag-region></div>
    <header class="library-header">
      <div class="library-title">
        <h1>Printing Press</h1>
        <p class="quiet">
          {projects.length === 0
            ? 'A reader and compiler for LaTeX and Markdown documents'
            : librarySummary}
        </p>
      </div>
      <div class="library-actions">
        <!-- Shows what it will do rather than where it is: a moon to go dark. -->
        <button
          class="theme-toggle"
          onclick={() => theme.toggle()}
          title={dark ? 'Light theme' : 'Dark theme'}
          aria-label={dark ? 'Switch to the light theme' : 'Switch to the dark theme'}
        >
          {#if dark}
            <Sun size={16} strokeWidth={1.75} aria-hidden="true" />
          {:else}
            <Moon size={16} strokeWidth={1.75} aria-hidden="true" />
          {/if}
        </button>
        <button
          class="theme-toggle"
          onclick={openEditorSettings}
          title="Settings"
          aria-label="Settings"
        >
          <Settings size={16} strokeWidth={1.75} aria-hidden="true" />
        </button>
        <button onclick={chooseDocument} disabled={busy}>
          {busy ? 'Opening…' : 'Open document'}
        </button>
      </div>
    </header>

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
        <section class="shelf" aria-labelledby="pinned-heading">
          <h2 class="section-title" id="pinned-heading">Pinned</h2>
          <div class="pinned-row">
          {#each pinned as project (project.id)}
            <article class="pinned-card">
              <button
                class="pinned-open"
                onclick={() => openProject(project)}
                oncontextmenu={(event) => openMenu(event, projectMenu(project))}
                disabled={busy || !project.available}
              >
                <span class="pinned-thumb" class:inverted={dark}>
                  {#if project.artifact}
                    <PdfThumbnail
                      artifact={project.artifact}
                      width={PINNED_THUMB_WIDTH}
                      invert={dark}
                    />
                  {:else}
                    <span class="missing-thumbnail">No PDF</span>
                  {/if}
                </span>
                <!-- The name and nothing else. A shelf is read by its covers;
                     what each document is and how it built is the grid's job. -->
                <strong class="name pinned-name" title={project.documentPath}>
                  {project.name}
                </strong>
              </button>
            </article>
          {/each}
          </div>
        </section>
      {/if}

      <section class="grid-section" aria-labelledby="all-heading">
        <h2 class="section-title" id="all-heading">All documents</h2>
        <div class="project-grid">
        {#each unpinned as project (project.id)}
          {@const state = cardState(project)}
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
              <span class="project-thumb" class:inverted={dark}>
                {#if project.artifact}
                  <PdfThumbnail
                    artifact={project.artifact}
                    width={GRID_THUMB_WIDTH}
                    invert={dark}
                  />
                {:else}
                  <span class="missing-thumbnail">No PDF</span>
                {/if}
              </span>
              <span class="project-lines">
                <strong class="name" title={project.documentPath}>{project.name}</strong>
                <!-- Which document this is, when the name alone does not say.
                     `main.tex` and `paper.tex` are what half the world calls
                     its source, so the folder is what tells two of them apart
                     — and the extension carries what the old `LATEX` line used
                     to, for nothing, on a line that was needed anyway. -->
                <span class="where" title={project.documentPath}
                  >{#if project.location}<span class="where-folder">{project.location}/</span
                    >{/if}<span class="where-file">{project.fileName}</span></span
                >
                <span class="state {state.tone}">{state.text}</span>
              </span>
            </button>
          </article>
        {/each}
        </div>
      </section>
    {/if}
  </main>
{/if}

{#if dropping}
  <!-- Over the window rather than around the library, which is only as tall as
       the documents in it. Says what will happen and takes no part in it. -->
  <div class="drop-veil" aria-hidden="true"></div>
{/if}

{#if peek}
  <!-- Dismissed by clicking anywhere else, including the next cmd-click, which
       lands on the backdrop and reaches the page underneath it. -->
  <button class="menu-backdrop" aria-label="Close the source" onclick={() => (peek = null)}
  ></button>
  <aside class="peek" bind:this={peekElement} transition:fade={{ duration: 100 }}>
    {#if peek.source}
      <header>
        <span class="peek-where">{peek.source.file}:{peek.source.startLine}{peek.source.endLine >
          peek.source.startLine
            ? `-${peek.source.endLine}`
            : ''}</span>
        <span class="quiet peek-version">{peek.version}</span>
        <button class="link" onclick={copyPeek}>Copy</button>
      </header>
      <pre>{peek.source.text}</pre>
    {:else if peek.empty}
      <p class="quiet peek-nothing">
        Nothing here comes from the document — this is the class file or the preamble.
      </p>
    {:else}
      <p class="quiet peek-nothing">Looking…</p>
    {/if}
  </aside>
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
  <dialog use:modal={() => (choosing = null)} aria-labelledby="choose-title">
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
  <dialog use:modal={() => (snapshotOpen = false)} aria-labelledby="snapshot-title">
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
  <dialog use:modal={() => (renaming = null)} aria-labelledby="rename-title">
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
        }}
      />
    </label>
    <div class="dialog-actions">
      <button onclick={() => (renaming = null)} disabled={busy}>Cancel</button>
      <button onclick={saveRename} disabled={busy || !renameTitle.trim()}>Save</button>
    </div>
  </dialog>
{/if}

{#if editorOpen}
  <dialog use:modal={() => (editorOpen = false)} aria-labelledby="editor-title">
    <h2 id="editor-title">Editor</h2>
    <p class="quiet">
      What the Editor button runs. Press opens the document and has nothing more
      to do with it — the folder is watched either way, so a save rebuilds
      whoever wrote it.
    </p>
    <label>
      Command
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:value={editorCommand}
        autofocus
        spellcheck="false"
        autocapitalize="off"
        autocorrect="off"
        onkeydown={(event) => {
          if (event.key === 'Enter') saveEditorCommand();
        }}
      />
    </label>
    <p class="quiet">
      <code>{'{file}'}</code> is the document and <code>{'{dir}'}</code> is its folder. Quote a word
      that has a space in it. Left empty, Press hands the document to the system and lets it open
      whatever you have set for that kind of file.
    </p>
    {#if editorDefault && editorCommand.trim() !== editorDefault.suggested}
      <p class="quiet">
        Suggested for this machine:
        <button class="link" onclick={() => (editorCommand = editorDefault?.suggested ?? '')}>
          <code>{editorDefault.suggested}</code>
        </button>
      </p>
    {/if}
    <div class="dialog-actions">
      <button onclick={() => (editorOpen = false)} disabled={busy}>Cancel</button>
      <button onclick={saveEditorCommand} disabled={busy}>Save</button>
    </div>
  </dialog>
{/if}

{#if settingsFor}
  <dialog use:modal={() => (settingsFor = null)} aria-labelledby="settings-title">
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
  <dialog use:modal={() => (confirmDelete = null)} aria-labelledby="delete-title">
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

{#if confirmDiscard}
  <dialog use:modal={() => (confirmDiscard = null)} aria-labelledby="discard-title">
    <h2 id="discard-title">Discard “{confirmDiscard.title}”?</h2>
    <p>
      This version and the PDF built from it are deleted. Your project folder is not touched, and
      the other versions stay as they are.
    </p>
    <div class="dialog-actions">
      <button onclick={() => (confirmDiscard = null)} disabled={busy}>Cancel</button>
      <button class="danger" onclick={discardVersion} disabled={busy}>Discard</button>
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
    border-radius: var(--radius-xs);
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
    /* The one column is pinned to the window. A grid's default `auto` track is
       sized to its widest item, and a page zoomed past the width of the window
       is exactly that: the track grew to the document, the footer stretched
       with it, and the viewer's own right edge — scrollbar included — ended up
       off screen. Worse, the viewer then had no width to scroll within, so
       there was nothing left to pan sideways. */
    grid-template-columns: minmax(0, 1fr);
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

  .bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
    padding: 0.25rem 0.5rem;
    font-size: var(--fs-card);
  }

  .bar.bottom {
    position: relative;
    border-top: var(--bw) solid var(--line);
    background: var(--card-2);
    color: var(--ink-3);
  }

  /* Scaled rather than resized: the transition then runs on the compositor, so
     a bar that updates on every shipped page does not lay out the footer with
     it. */
  .progress {
    position: absolute;
    top: calc(var(--bw) * -1);
    left: 0;
    width: 100%;
    height: 2px;
    transform: scaleX(var(--filled));
    transform-origin: left center;
    background: var(--accent);
    transition: transform 220ms ease-out;
    pointer-events: none;
  }

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

  /* The history sits beside the document rather than over it, so a version can
     be picked while still reading. */
  .document {
    display: flex;
    position: relative;
    min-height: 0;
    /* With the track pinned above, this is what lets the row sit inside it
       rather than being pushed wide by the document it holds. */
    min-width: 0;
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
    box-shadow: var(--shadow-lg);
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
    user-select: none;
    -webkit-user-select: none;
  }

  /* Title and age are one line. The age is pushed to the far end and never
     shrinks; a long title wraps under itself rather than pressing on it. */
  .version-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
    min-width: 0;
  }

  .version-head strong {
    min-width: 0;
    line-height: 1.35;
    overflow-wrap: anywhere;
  }

  .version-head .stamp {
    flex: none;
    font-size: var(--fs-meta);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  /* Notes run under the title. Three lines is enough to tell two versions
     apart; past that the version has to be opened. */
  .version-open .body {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    font-size: var(--fs-meta);
    line-height: 1.4;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
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
    color: var(--warning);
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
    font-family: var(--font-mono);
  }

  .starting {
    height: 100vh;
    background: var(--paper);
  }

  /* -- library --------------------------------------------------------- */

  .library {
    /* The gutter is on the sections, not here: the pinned shelf runs its rules
       the full width of the window. */
    padding: 0 0 var(--gutter);
    background: var(--paper);
  }

  /* Only used by the library, where the heading needs to clear the lights. */
  .titlebar {
    height: 2.375rem;
  }

  .library-actions,
  .dialog-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  /* The title and the button hang off the same baseline rather than being
     centred against each other. At a narrow window the button drops below the
     title instead of squeezing it off the screen. */
  .library-header {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: var(--space-xl);
    flex-wrap: wrap;
    padding: 0 var(--gutter) var(--space-xl);
  }

  /* The title block only. Named rather than matched as `> div`, which also
     catches the actions beside it and stood them on end. */
  .library-title {
    display: flex;
    flex-direction: column;
    /* The title sets line-height 1, so most of the space between the two lines
       is this gap; the sentence below brings a little of its own. */
    gap: var(--space-xs);
  }

  .library-header h1 {
    margin: 0;
    font-size: var(--fs-title);
    font-weight: var(--fw-title);
    line-height: 1;
    letter-spacing: var(--tracking-title);
    color: var(--ink);
  }

  /* A sentence about what Press is, not a label: read once and then ignored, so
     it sits at body size in the quietest ink and takes no tracking of its own. */
  .library-header p {
    margin: 0;
    font-size: var(--fs-card);
    font-weight: 400;
    line-height: 1.35;
    letter-spacing: normal;
    color: var(--ink-3);
  }

  /* The one solid control on the page: accent fill, white text, and flat — the
     only thing here that is coloured rather than shadowed. */
  .library-actions button {
    padding: 0.5625rem 0.9375rem;
    border: 0;
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--on-accent);
    font-family: var(--font-sans);
    font-size: var(--fs-body);
    font-weight: 500;
    line-height: 1;
    cursor: pointer;
    transition: background var(--duration);
  }

  .library-actions button:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  /* The theme is a preference, not an action, so it is an icon in the quietest
     ink beside the one filled control rather than a second thing competing with
     it. Two classes, so it outranks the rule above without disowning it. */
  .library-actions .theme-toggle {
    display: grid;
    place-items: center;
    padding: 0.5rem;
    border-radius: var(--radius);
    background: none;
    color: var(--ink-3);
    transition: background var(--duration), color var(--duration);
  }

  .library-actions .theme-toggle:hover {
    background: var(--paper-2);
    color: var(--ink);
  }

  /* The band, which holds its own name as well as the documents on it.
     No side padding: the row inside it scrolls, and a scroller has to reach
     the window's edges or documents disappear under a margin instead of
     running off the end. The gutter moves onto the things inside. */
  .shelf {
    /* Even top and bottom. The deeper foot was there to balance a two-line
       caption; with one line the shelf can close up. */
    padding: var(--shelf-pad-y) 0;
    border-top: var(--bw) solid var(--line);
    border-bottom: var(--bw) solid var(--line);
    background: var(--shelf);
  }

  .shelf .section-title {
    padding-inline: var(--gutter);
  }

  /* A shelf you push along rather than one that grows downwards. It holds one
     row however many are pinned, so pinning a seventh document cannot push the
     library about — and it scrolls itself rather than the window, which is the
     honest fix for the overflow the wrap was covering up.
     `scroll-padding` is what makes a card come to rest on the gutter rather
     than against the glass, and it is the reason the padding is here and not on
     the band: the two have to agree. */
  .pinned-row {
    display: flex;
    flex-wrap: nowrap;
    gap: var(--shelf-gap);
    margin: 0;
    padding-inline: var(--gutter);
    overflow-x: auto;
    overflow-y: hidden;
    scroll-snap-type: x mandatory;
    scroll-padding-inline: var(--gutter);
    /* A sideways swipe on a trackpad is a back-navigation gesture before it is
       anything else. This is what stops the shelf handing one over. */
    overscroll-behavior-x: contain;
    scroll-behavior: smooth;
  }

  /* Hidden here alone. Everywhere else in Press a scrollbar says how much more
     there is; under a row of three book covers it is a permanent 12px rule
     across the page. What says there is more here is the next cover, showing
     at the edge because the snap leaves it there. */
  .pinned-row {
    scrollbar-width: none;
  }

  .pinned-row::-webkit-scrollbar {
    height: 0;
  }

  .pinned-card {
    scroll-snap-align: start;
  }

  /* Smooth scrolling is a preference, not a decoration. */
  @media (prefers-reduced-motion: reduce) {
    .pinned-row {
      scroll-behavior: auto;
    }
  }

  .grid-section {
    padding: var(--space-2xl) var(--gutter) 0;
  }

  /* The uppercase eyebrow, moved. It used to sit on every card saying `LATEX`,
     which is the loudest treatment a card has spent on the one thing about a
     document that is almost never in question. Naming a section is what that
     treatment is for: it is structure rather than content, and there are two of
     them on the page instead of one per document. */
  .section-title {
    margin: 0 0 var(--section-gap);
    color: var(--ink-3);
    font-size: var(--fs-label);
    font-weight: var(--fw-label);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }

  /* No card chrome: the page image is the card. */
  .pinned-card {
    border: 0;
    border-radius: 0;
    overflow: visible;
    width: var(--shelf-thumb-w);
    flex: none;
  }

  .pinned-open {
    display: flex;
    flex-direction: column;
    gap: var(--space-md);
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

  /* The radius and the shadow live on the page image rather than on the card
     around it, because the page image is what actually looks like paper. */
  .pinned-thumb {
    display: block;
    width: var(--shelf-thumb-w);
    height: var(--shelf-thumb-h);
    overflow: hidden;
    border-radius: var(--radius);
    background: var(--sheet);
    box-shadow: var(--shadow-md);
    transition: box-shadow var(--duration);
  }

  /* Settling rather than lifting: under the pointer the page presses down onto
     the shelf and its shadow tightens. */
  .pinned-open:hover:not(:disabled) .pinned-thumb {
    box-shadow: var(--shadow-sm);
  }

  /* A thumbnail is a page, so it takes the same sheet the viewer would give it
     — the drawn one, once the drawing is inverted. Without this the paper the
     renderer produces would sit on a white card for as long as it took to
     arrive, which is the flash the viewer is careful to avoid. */
  .pinned-thumb.inverted,
  .project-thumb.inverted {
    background: var(--sheet-inverted);
  }

  /* Centred under the page image, and the only line on the card. */
  .pinned-name {
    width: 100%;
    min-width: 0;
    text-align: center;
  }

  /* Cards keep their size and the row keeps as many as it can hold. A shelf
     that gets shorter holds fewer things; it does not shrink them. */
  .project-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, var(--grid-card-w));
    justify-content: start;
    gap: var(--grid-gap-y) var(--grid-gap-x);
  }

  .project-card {
    display: flex;
    align-items: center;
    gap: var(--space-md);
    padding: var(--row-pad);
    margin: calc(var(--row-pad) * -1);
    border: 0;
    border-radius: var(--radius-sm);
    overflow: visible;
    transition: background var(--duration);
  }

  /* The row lifts to card white under the pointer. Nothing else marks it: the
     thumbnail keeps its own shadow either way. */
  .project-card:hover {
    background: var(--card);
  }

  .project-open {
    /* The text block is shorter than the page image, so it is centred against
       it rather than hung from the top edge. */
    align-items: center;
    gap: var(--space-md);
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
    display: block;
    flex: none;
    width: var(--grid-thumb-w);
    height: var(--grid-thumb-h);
    overflow: hidden;
    border-radius: var(--radius-sm);
    background: var(--sheet);
    box-shadow: var(--shadow-sm);
  }

  /* Three lines, and each is told apart from the one above it by a different
     thing: the name by weight and ink, the path by family, the state by colour.
     They used to differ by letter-spacing alone, two of them sharing a size and
     a colour, which is why they read as one block with a loud first line. */
  .project-lines {
    gap: var(--space-xs);
    display: grid;
    flex: 1;
    min-width: 0;
  }

  /* Where the document is, and — through the extension it ends in — what it is
     written in. Monospaced because it is a path, which is also what tells it
     apart from the line below without spending a size or a colour on doing so.
     A narrow column clips this rather than letting it push on its neighbour:
     the name above it is the part that has to survive. */
  .where {
    display: flex;
    min-width: 0;
    overflow: hidden;
    font-family: var(--font-mono);
    font-size: var(--fs-meta);
    line-height: 1.2;
    color: var(--ink-3);
    white-space: nowrap;
  }

  /* The folder gives way and the file name does not. Truncating this line from
     the right would eat the extension, and the extension is the half that says
     what the document is written in — the whole reason this line replaced the
     `LATEX` one. So the folder ellipsises and the file always survives. */
  .where-folder {
    min-width: 0;
    flex: 0 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .where-file {
    flex: none;
  }

  /* The state of the document, and the faintest tier — the only line that
     changes on its own. Quiet is the ordinary case: a colour here means
     something wants doing. */
  .state {
    overflow: hidden;
    font-size: var(--fs-meta);
    line-height: 1.2;
    color: var(--ink-3);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .state.busy {
    color: var(--warning);
  }

  .state.bad {
    color: var(--danger);
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

  /* The parent sets the box, so this only has to fill it. */
  .missing-thumbnail {
    display: grid;
    width: 100%;
    height: 100%;
    place-items: center;
    background: var(--paper-3);
    color: var(--ink);
    font-size: var(--fs-label);
    text-align: center;
  }

  /* The gutter is on each section rather than on .library, so this brings its
     own. */
  .empty-library {
    padding-inline: var(--gutter);
    display: grid;
    place-items: center;
    align-content: center;
    min-height: 55vh;
    text-align: center;
  }

  /* The window while a document is being held over it. An accent edge and the
     faintest wash of the same colour — enough to say the drop will land, and
     little enough that the library is still readable underneath it. */
  .drop-veil {
    position: fixed;
    inset: 0.5rem;
    z-index: 45;
    border: var(--bw-2) solid var(--accent);
    border-radius: var(--radius);
    background: var(--accent-wash);
    pointer-events: none;
  }

  /* -- peek --------------------------------------------------------------- */

  /* Sits where it was asked for. Where exactly is measured against the viewer's
     own edges once the source is in — see `placePeek` — which is also what sets
     the size limits and reveals it. Hidden until then, because a popover that
     is placed twice reads as a flinch. */
  .peek {
    position: fixed;
    top: 0;
    left: 0;
    z-index: 32;
    display: flex;
    flex-direction: column;
    width: max-content;
    visibility: hidden;
    background: var(--card);
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius);
    box-shadow: var(--shadow-xl);
    overflow: hidden;
  }

  .peek header {
    display: flex;
    flex: none;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.35rem 0.5rem 0.35rem 0.65rem;
    border-bottom: var(--bw) solid var(--line);
    background: var(--paper-2);
    font-size: var(--fs-meta);
  }

  .peek-where {
    color: var(--ink-2);
    font-family: var(--font-mono);
    overflow-wrap: anywhere;
  }

  /* Which version this source is — the whole point when two of them are open
     side by side. */
  .peek-version {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: right;
  }

  /* The source scrolls inside whatever height the placement allowed, rather
     than pushing the popover past the edge it was fitted to. */
  .peek pre {
    min-height: 0;
    margin: 0;
    padding: 0.6rem 0.65rem;
    overflow: auto;
    font-family: var(--font-mono);
    font-size: var(--fs-card);
    line-height: 1.45;
    tab-size: 2;
    /* Source is read as it was written: wrapped, never reflowed. */
    white-space: pre;
  }

  .peek-nothing {
    margin: 0;
    padding: 0.6rem 0.75rem;
    font-size: var(--fs-menu);
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
    box-shadow: var(--shadow-lg);
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
    background: var(--danger-tint);
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

  /* Positioned rather than left to the user agent's centring, which the reader's
     100vh grid does not agree with. `right` and `margin` are reset with it: the
     user agent sets `inset-inline-end: 0` and `margin: auto` on every dialog,
     and left:50% against right:0 leaves the box centred in the right half of the
     window rather than in the window.

     Opened with `showModal` — see `$lib/modal` — so this is in the top layer and
     the z-index only settles it against the menus. */
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
    box-shadow: var(--shadow-xl);
    color: var(--ink);
    font-size: var(--fs-menu);
  }

  /* Enough to say that what is behind is not the thing to click, and no more:
     a cutout resting on the desk, not a spotlight on a stage. */
  dialog::backdrop {
    background: var(--backdrop);
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

  /* The app's own focus, not the browser's: the field lifts to card white, its
     edge takes the accent, and a soft ring of the same colour sits under it.
     The global blue outline is suppressed here — inside a dialog it is the
     only blue on screen, and it reads as a browser artefact. */
  dialog input:focus,
  dialog select:focus,
  dialog textarea:focus {
    outline: none;
    border-color: var(--accent);
    background: var(--card);
    box-shadow: 0 0 0 3px var(--accent-wash);
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
    transition: background var(--duration);
  }

  .dialog-actions button:hover:not(:disabled) {
    background: var(--paper-2);
    color: var(--ink);
  }

  .dialog-actions button:last-child {
    border-color: transparent;
    background: var(--accent);
    color: var(--on-accent);
  }

  .dialog-actions button:last-child:hover:not(:disabled) {
    background: var(--accent-hover);
    color: var(--on-accent);
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
