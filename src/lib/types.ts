export type Engine = 'pdflatex' | 'xelatex' | 'lualatex';

/** What a project is written in. Markdown reaches latexmk through pandoc. */
export type DocumentKind = 'latex' | 'markdown';

export const ENGINES: Engine[] = ['pdflatex', 'xelatex', 'lualatex'];

export type BuildStatus =
  | 'never'
  | 'queued'
  | 'running'
  | 'success'
  | 'error'
  | 'interrupted';

/** `worktree`, or `snapshot:<revision>` once Press keeps a history. */
export type SourceRef = string;

export const WORKTREE: SourceRef = 'worktree';

export type Severity = 'error' | 'warning';

export type Diagnostic = {
  /** Project-relative when it could be resolved inside the project. */
  file: string | null;
  line: number | null;
  severity: Severity;
  message: string;
};

export type BuildState = {
  sourceRef: SourceRef;
  status: BuildStatus;
  startedAt: number | null;
  finishedAt: number | null;
  durationMs: number | null;
  errorSummary: string | null;
  diagnostics: Diagnostic[];
};

export type ArtifactSummary = {
  id: number;
  projectId: number;
  sourceRef: SourceRef;
  engine: Engine;
  pageCount: number | null;
  byteSize: number;
  builtAt: number;
  /** Bumped when the bytes are replaced; part of the viewer's cache key. */
  revision: number;
};

/** A project plus the state of its working tree. */
export type ProjectSummary = {
  id: number;
  name: string;
  rootPath: string;
  mainFile: string;
  workingDirectory: string;
  kind: DocumentKind;
  engine: Engine;
  createdAt: number;
  lastOpenedAt: number;
  build: BuildState;
  artifact: ArtifactSummary | null;
  pathAvailable: boolean;
  mainFileAvailable: boolean;
};

/** A path handed to Press from outside: the command line, or `:Press`. */
export type OpenRequest = {
  path: string;
  projectId: number | null;
  report: DiscoveryReport | null;
  message: string | null;
};

/** A stored version of a project's source. */
export type SnapshotSummary = {
  id: number;
  projectId: number;
  /** Manifest hash; two snapshots of identical content share it, and its build. */
  revision: string;
  title: string;
  body: string | null;
  createdAt: number;
  fileCount: number;
  byteSize: number;
};

/** One row of the history: the working tree, or a snapshot. */
export type VersionSummary = {
  sourceRef: SourceRef;
  title: string;
  snapshot: SnapshotSummary | null;
  build: BuildState;
  artifact: ArtifactSummary | null;
};

/** A page's size in PDF points. */
export type PageSize = {
  width: number;
  height: number;
};

/** One word and where it sits, in PDF points from the page's top left. */
export type TextBox = {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
  line: number;
};

export type SearchHit = {
  page: number;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type MainCandidate = {
  relativePath: string;
  kind: DocumentKind;
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
  /** Every latexmk configuration file in the folder; these are executable Perl. */
  latexmkrcPaths: string[];
  detectedEngine: Engine | null;
  warnings: string[];
  toolchain: {
    latexmk: ToolInfo;
    pandoc: ToolInfo;
    neovim: ToolInfo;
  };
};

export type EditorLaunchResult = {
  status: 'launched' | 'connected';
  socketPath: string;
  message: string;
};

/** Parsed from latexmk's own output, not from a timer. */
export type BuildProgress = {
  buildId: number;
  projectId: number;
  sourceRef: SourceRef;
  stage: string;
  pass: number | null;
  page: number | null;
  expectedPages: number | null;
};

export type BuildUpdate = {
  buildId: number | null;
  projectId: number;
  sourceRef: SourceRef;
  build: BuildState;
  artifact: ArtifactSummary | null;
};

export type WatcherError = {
  projectId: number;
  message: string;
};

export type CommandError = {
  code?: string;
  message?: string;
};
