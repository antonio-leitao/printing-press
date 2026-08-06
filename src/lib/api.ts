import { invoke } from '@tauri-apps/api/core';
import type {
  DiscoveryReport,
  EditorLaunchResult,
  Engine,
  OpenRequest,
  PageSize,
  ProjectSummary,
  SearchHit,
  SnapshotSummary,
  SourceRef,
  TextBox,
  VersionSummary
} from '$lib/types';

export const api = {
  listProjects: () => invoke<ProjectSummary[]>('list_projects'),

  inspectProject: (rootPath: string) =>
    invoke<DiscoveryReport>('inspect_project', { rootPath }),

  /** Inspects one named file: this document, in this folder, no discovery. */
  inspectDocument: (filePath: string) =>
    invoke<DiscoveryReport>('inspect_document', { filePath }),

  addProject: (rootPath: string, mainFile: string, engineOverride?: Engine) =>
    invoke<ProjectSummary>('add_project', { rootPath, mainFile, engineOverride }),

  openProject: (projectId: number) =>
    invoke<ProjectSummary>('open_project', { projectId }),

  closeProject: () => invoke<void>('close_project'),

  /** Defaults to the working tree; a source reference picks a version. */
  buildProject: (projectId: number, sourceRef?: SourceRef) =>
    invoke<number>('build_project', { projectId, sourceRef }),

  renameProject: (projectId: number, name: string) =>
    invoke<ProjectSummary>('rename_project', { projectId, name }),

  updateProjectSettings: (
    projectId: number,
    settings: { mainFile?: string; engineOverride?: Engine }
  ) => invoke<ProjectSummary>('update_project_settings', { projectId, ...settings }),

  deleteProject: (projectId: number) =>
    invoke<void>('delete_project', { projectId }),

  /** Stores the project's source as it is now, under a title. */
  createSnapshot: (projectId: number, title: string, body?: string) =>
    invoke<SnapshotSummary>('create_snapshot', { projectId, title, body }),

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

  searchDocument: (artifactId: number, needle: string) =>
    invoke<SearchHit[]>('search_document', { artifactId, needle }),

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
