export type ProjectSummary = {
  id: number;
  name: string;
  rootPath: string;
  mainFile: string;
  workingDirectory: string;
  engine: string;
  buildStatus: 'never' | 'building' | 'success' | 'error' | 'interrupted';
  lastBuildAt: number | null;
  lastBuildDurationMs: number | null;
  lastError: string | null;
  artifactRevision: number;
  hasPdf: boolean;
  pathAvailable: boolean;
};

export type MainCandidate = {
  relativePath: string;
  score: number;
  reasons: string[];
};

export type ToolInfo = {
  available: boolean;
  path: string | null;
  version: string | null;
};

export type DiscoveryReport = {
  rootPath: string;
  projectName: string;
  texFileCount: number;
  candidates: MainCandidate[];
  recommendedMain: string | null;
  requiresSelection: boolean;
  hasLatexmkrc: boolean;
  warnings: string[];
  toolchain: {
    latexmk: ToolInfo;
    neovim: ToolInfo;
  };
};

export type EditorLaunchResult = {
  status: 'launched' | 'connected';
  socketPath: string;
  message: string;
};

export type CommandError = {
  code?: string;
  message?: string;
};
