import { invoke } from '@tauri-apps/api/core';
import type {
  EditorLaunchResult,
  Engine,
  OpenRequest,
  PageSize,
  ProjectSummary,
  SearchHit,
  SnapshotOutcome,
  SourcePeek,
  SourceRef,
  TextBox,
  VersionSummary
} from '$lib/types';

export const api = {
  listProjects: () => invoke<ProjectSummary[]>('list_projects'),

  /** The one way in: a file or a directory, and the documents it means. */
  resolvePath: (path: string) => invoke<OpenRequest>('resolve_path', { path }),

  addProject: (documentPath: string, name?: string, engineOverride?: Engine) =>
    invoke<ProjectSummary>('add_project', { documentPath, name, engineOverride }),

  openProject: (projectId: number) =>
    invoke<ProjectSummary>('open_project', { projectId }),

  closeProject: () => invoke<void>('close_project'),

  /** Defaults to the working tree; a source reference picks a version. */
  buildProject: (projectId: number, sourceRef?: SourceRef) =>
    invoke<number>('build_project', { projectId, sourceRef }),

  renameProject: (projectId: number, name: string) =>
    invoke<ProjectSummary>('rename_project', { projectId, name }),

  /** Discards every cached PDF: versions built by different engines are not comparable. */
  setProjectEngine: (projectId: number, engineOverride: Engine) =>
    invoke<ProjectSummary>('set_project_engine', { projectId, engineOverride }),

  /** Keeps a project at the top of the library. Touches nothing else. */
  setProjectPinned: (projectId: number, pinned: boolean) =>
    invoke<ProjectSummary>('set_project_pinned', { projectId, pinned }),

  deleteProject: (projectId: number) =>
    invoke<void>('delete_project', { projectId }),

  /** Stores the project's source as it is now, under a title. */
  createSnapshot: (projectId: number, title: string, body?: string) =>
    invoke<SnapshotOutcome>('create_snapshot', { projectId, title, body }),

  /** The working tree pinned at the top, then every snapshot. */
  listVersions: (projectId: number) =>
    invoke<VersionSummary[]>('list_versions', { projectId }),

  renameSnapshot: (snapshotId: number, title: string) =>
    invoke<void>('rename_snapshot', { snapshotId, title }),

  deleteSnapshot: (snapshotId: number) =>
    invoke<void>('delete_snapshot', { snapshotId }),

  /** Every page's size, cheap enough to lay out a whole document up front. */
  pageLayout: (artifactId: number) =>
    invoke<PageSize[]>('page_layout', { artifactId }),

  /** Word boxes for the selection overlay. */
  pageWords: (artifactId: number, page: number) =>
    invoke<TextBox[]>('page_words', { artifactId, page }),

  /**
   * The source behind a point on a page, in PDF points from its top left.
   * Null when the point resolves to something outside the document — a class
   * file, or pandoc's own preamble.
   */
  peekSource: (artifactId: number, page: number, x: number, y: number) =>
    invoke<SourcePeek | null>('peek_source', { artifactId, page, x, y }),

  searchDocument: (artifactId: number, needle: string) =>
    invoke<SearchHit[]>('search_document', { artifactId, needle }),

  /** Copies a built PDF into Downloads. Returns where it was written. */
  exportArtifact: (artifactId: number) =>
    invoke<string>('export_artifact', { artifactId }),

  getBuildLog: (projectId: number, sourceRef?: SourceRef) =>
    invoke<string>('get_build_log', { projectId, sourceRef }),

  launchNeovim: (projectId: number) =>
    invoke<EditorLaunchResult>('launch_neovim', { projectId }),

  editorStatus: (projectId: number) =>
    invoke<string>('editor_status', { projectId }),

  /** Collects a path Press was launched with. Taking it clears it. */
  takePendingOpen: () => invoke<OpenRequest | null>('take_pending_open'),

  /** Anything Press wants to say about how it started. Said once. */
  takeStartupNotice: () => invoke<string | null>('take_startup_notice')
};

export function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === 'string') return message;
  }
  return 'An unexpected error occurred.';
}
