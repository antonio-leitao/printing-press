import { invoke } from '@tauri-apps/api/core';
import type { DiscoveryReport, EditorLaunchResult, ProjectSummary } from '$lib/types';

export const api = {
  listProjects: () => invoke<ProjectSummary[]>('list_projects'),
  inspectProject: (rootPath: string) =>
    invoke<DiscoveryReport>('inspect_project', { rootPath }),
  addProject: (rootPath: string, mainFile: string) =>
    invoke<ProjectSummary>('add_project', { rootPath, mainFile }),
  activateProject: (projectId: number) =>
    invoke<ProjectSummary>('activate_project', { projectId }),
  deactivateProject: () => invoke<void>('deactivate_project'),
  rebuildProject: (projectId: number) =>
    invoke<void>('rebuild_project', { projectId }),
  readProjectPdf: async (projectId: number) => {
    const response = await invoke<ArrayBuffer>('read_project_pdf', { projectId });
    return new Uint8Array(response);
  },
  getBuildLog: (projectId: number) =>
    invoke<string>('get_build_log', { projectId }),
  launchNeovim: (projectId: number) =>
    invoke<EditorLaunchResult>('launch_neovim', { projectId }),
  editorStatus: (projectId: number) =>
    invoke<string>('editor_status', { projectId })
};

export function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === 'string') return message;
  }
  return 'An unexpected error occurred.';
}
