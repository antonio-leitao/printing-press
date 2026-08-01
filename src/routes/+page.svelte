<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { open } from '@tauri-apps/plugin-dialog';
  import PdfThumbnail from '$lib/PdfThumbnail.svelte';
  import PdfViewer from '$lib/PdfViewer.svelte';
  import { api, errorMessage } from '$lib/api';
  import type { DiscoveryReport, ProjectSummary } from '$lib/types';

  let projects = $state<ProjectSummary[]>([]);
  let activeProject = $state<ProjectSummary | null>(null);
  let discovery = $state<DiscoveryReport | null>(null);
  let selectedMain = $state('');
  let busy = $state(false);
  let error = $state('');
  let notice = $state('');
  let buildLog = $state('');
  let editorState = $state('closed');

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      unlisten = await listen<ProjectSummary>('project-updated', (event) => {
        const project = event.payload;
        const index = projects.findIndex((item) => item.id === project.id);
        projects =
          index === -1
            ? [project, ...projects]
            : projects.map((item) => (item.id === project.id ? project : item));
        if (activeProject?.id === project.id) {
          activeProject = project;
          if (project.buildStatus !== 'building') buildLog = '';
        }
      });
      if (!disposed) await refreshProjects();
    })();

    const editorPoll = window.setInterval(() => {
      const id = activeProject?.id;
      if (id === undefined) return;
      void api
        .editorStatus(id)
        .then((status) => (editorState = status))
        .catch(() => (editorState = 'closed'));
    }, 3000);

    return () => {
      disposed = true;
      unlisten?.();
      window.clearInterval(editorPoll);
    };
  });

  async function refreshProjects() {
    try {
      projects = await api.listProjects();
    } catch (reason) {
      error = errorMessage(reason);
    }
  }

  async function chooseProjectFolder() {
    clearMessages();
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: 'Add a LaTeX project'
      });
      if (!selected || Array.isArray(selected)) return;
      busy = true;
      const report = await api.inspectProject(selected);
      if (!report.toolchain.latexmk.available) {
        error =
          'latexmk was not found. Install a TeX distribution or add latexmk to PATH.';
        return;
      }
      if (report.candidates.length === 0) {
        error = report.warnings[0] ?? 'No LaTeX document root was found in this folder.';
        return;
      }
      if (report.recommendedMain && !report.requiresSelection && !report.hasLatexmkrc) {
        await saveAndOpenProject(report, report.recommendedMain);
      } else {
        discovery = report;
        selectedMain = report.recommendedMain ?? report.candidates[0]?.relativePath ?? '';
      }
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  async function confirmProject() {
    if (!discovery || !selectedMain) return;
    busy = true;
    clearMessages();
    try {
      await saveAndOpenProject(discovery, selectedMain);
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  async function saveAndOpenProject(report: DiscoveryReport, mainFile: string) {
    const project = await api.addProject(report.rootPath, mainFile);
    discovery = null;
    selectedMain = '';
    projects = [project, ...projects.filter((item) => item.id !== project.id)];
    await openProject(project);
  }

  async function openProject(project: ProjectSummary) {
    clearMessages();
    busy = true;
    try {
      activeProject = await api.activateProject(project.id);
      editorState = await api.editorStatus(project.id);
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
      await api.deactivateProject();
      activeProject = null;
      editorState = 'closed';
      buildLog = '';
      await refreshProjects();
    } catch (reason) {
      error = errorMessage(reason);
    } finally {
      busy = false;
    }
  }

  async function rebuild() {
    if (!activeProject) return;
    clearMessages();
    try {
      await api.rebuildProject(activeProject.id);
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

  async function loadBuildLog(event: Event) {
    if (!activeProject || !(event.currentTarget instanceof HTMLDetailsElement)) return;
    if (!event.currentTarget.open || buildLog) return;
    try {
      buildLog = await api.getBuildLog(activeProject.id);
    } catch (reason) {
      buildLog = errorMessage(reason);
    }
  }

  function clearMessages() {
    error = '';
    notice = '';
  }

  function buildDescription(project: ProjectSummary) {
    if (project.buildStatus === 'building') {
      return project.hasPdf ? 'Rebuilding — showing the last good PDF' : 'Building initial preview';
    }
    if (project.buildStatus === 'success') {
      const duration = project.lastBuildDurationMs
        ? ` in ${(project.lastBuildDurationMs / 1000).toFixed(1)}s`
        : '';
      return `Last build succeeded${duration}`;
    }
    if (project.buildStatus === 'error') return 'Build failed — the last good PDF is preserved';
    if (project.buildStatus === 'interrupted') return 'Previous build was interrupted';
    return 'Not built yet';
  }

  function lastBuilt(project: ProjectSummary) {
    if (!project.lastBuildAt) return 'Never built';
    return new Date(project.lastBuildAt * 1000).toLocaleString();
  }
</script>

{#if activeProject}
  <main class="project-view">
    <header class="project-header">
      <button onclick={returnToLibrary} disabled={busy}>Projects</button>
      <div class="project-title">
        <h1>{activeProject.name}</h1>
        <p><code>{activeProject.mainFile}</code> · {activeProject.engine}</p>
      </div>
      <button onclick={rebuild} disabled={busy || activeProject.buildStatus === 'building'}>
        Build
      </button>
      <button onclick={launchEditor} disabled={busy}>Launch Neovim</button>
    </header>

    <section class="status" aria-live="polite">
      <span>{buildDescription(activeProject)}</span>
      <span>Neovim: {editorState}</span>
    </section>

    {#if error}<p class="message error" role="alert">{error}</p>{/if}
    {#if notice}<p class="message" role="status">{notice}</p>{/if}
    {#if activeProject.lastError}
      <p class="message error" role="alert">{activeProject.lastError}</p>
    {/if}

    <section class="document">
      {#if activeProject.hasPdf}
        <PdfViewer
          projectId={activeProject.id}
          revision={activeProject.artifactRevision}
        />
      {:else}
        <div class="empty-document">
          {#if activeProject.buildStatus === 'building'}
            <progress aria-label="Building initial preview"></progress>
            <h2>Building the first preview</h2>
            <p>The PDF will appear here after latexmk produces a verified document.</p>
          {:else}
            <h2>No successful PDF yet</h2>
            <p>Fix the build error and save a source file, or choose Build.</p>
          {/if}
        </div>
      {/if}
    </section>

    <details class="build-log" ontoggle={loadBuildLog}>
      <summary>Build output</summary>
      <pre>{buildLog || 'Open this section after a build to inspect its output.'}</pre>
    </details>
  </main>
{:else}
  <main class="library">
    <header class="library-header">
      <div>
        <h1>Press</h1>
        <p>LaTeX projects and their last successful builds</p>
      </div>
      <button onclick={chooseProjectFolder} disabled={busy}>
        {busy ? 'Inspecting…' : 'Add project'}
      </button>
    </header>

    {#if error}<p class="message error" role="alert">{error}</p>{/if}
    {#if notice}<p class="message" role="status">{notice}</p>{/if}

    {#if projects.length === 0}
      <section class="empty-library">
        <h2>No projects yet</h2>
        <p>Add a folder containing a LaTeX document. Press will locate its main file and compile it.</p>
        <button onclick={chooseProjectFolder} disabled={busy}>Add project</button>
      </section>
    {:else}
      <section class="project-grid" aria-label="Saved projects">
        {#each projects as project (project.id)}
          <button
            class="project-card"
            onclick={() => openProject(project)}
            disabled={busy || !project.pathAvailable}
          >
            {#if project.hasPdf}
              {#key project.artifactRevision}
                <PdfThumbnail projectId={project.id} revision={project.artifactRevision} />
              {/key}
            {:else}
              <div class="missing-thumbnail">No compiled PDF</div>
            {/if}
            <strong>{project.name}</strong>
            <span>{project.mainFile}</span>
            <span>{lastBuilt(project)}</span>
            {#if !project.pathAvailable}<span>Folder unavailable</span>{/if}
          </button>
        {/each}
      </section>
    {/if}
  </main>
{/if}

{#if discovery}
  <dialog open aria-labelledby="main-file-title">
    <h2 id="main-file-title">
      {discovery.requiresSelection ? 'Choose the main document' : 'Confirm project'}
    </h2>
    {#if discovery.requiresSelection}
      <p>Several files could be compiled in <strong>{discovery.projectName}</strong>.</p>
    {:else}
      <p>Press found <strong>{selectedMain}</strong> in {discovery.projectName}.</p>
    {/if}
    {#if discovery.hasLatexmkrc}
      <p class="error" role="alert">
        This folder contains a latexmk configuration file, which is executable code. Add it only
        if you trust this project.
      </p>
    {/if}
    <label>
      Main TeX file
      <select bind:value={selectedMain}>
        {#each discovery.candidates as candidate}
          <option value={candidate.relativePath}>{candidate.relativePath}</option>
        {/each}
      </select>
    </label>
    {#if selectedMain}
      <ul>
        {#each discovery.candidates.find((candidate) => candidate.relativePath === selectedMain)?.reasons ?? [] as reason}
          <li>{reason}</li>
        {/each}
      </ul>
    {/if}
    <div class="dialog-actions">
      <button onclick={() => (discovery = null)} disabled={busy}>Cancel</button>
      <button onclick={confirmProject} disabled={busy || !selectedMain}>Add and build</button>
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
  }

  :global(button),
  :global(select) {
    font: inherit;
  }

  :global(button) {
    padding: 0.5rem 0.75rem;
  }

  h1,
  h2,
  p {
    margin-top: 0;
  }

  .library {
    padding: 1.5rem;
  }

  .library-header,
  .project-header,
  .status,
  .dialog-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .library-header,
  .status {
    justify-content: space-between;
  }

  .project-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 1rem;
  }

  .project-card {
    display: grid;
    gap: 0.4rem;
    padding: 0;
    overflow: hidden;
    text-align: left;
  }

  .project-card > :global(strong),
  .project-card > :global(span) {
    margin-inline: 0.75rem;
  }

  .project-card > :global(span:last-child) {
    margin-bottom: 0.75rem;
  }

  .missing-thumbnail {
    display: grid;
    min-height: 180px;
    place-items: center;
    background: #e7e7e7;
  }

  .empty-library,
  .empty-document {
    display: grid;
    place-items: center;
    align-content: center;
    min-height: 55vh;
    text-align: center;
  }

  .project-view {
    display: grid;
    grid-template-rows: auto auto auto minmax(0, 1fr) auto;
    height: 100vh;
  }

  .project-header,
  .status,
  .message,
  .build-log {
    margin: 0;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid #aaa;
  }

  .project-title {
    flex: 1;
    min-width: 0;
  }

  .project-title h1,
  .project-title p {
    overflow: hidden;
    margin: 0;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .document {
    min-height: 0;
  }

  .message.error {
    border-color: #a00;
  }

  .build-log pre {
    max-height: 12rem;
    overflow: auto;
    white-space: pre-wrap;
  }

  dialog {
    width: min(34rem, calc(100% - 2rem));
  }

  dialog label,
  dialog select {
    display: block;
    width: 100%;
  }

  .dialog-actions {
    justify-content: flex-end;
  }
</style>
