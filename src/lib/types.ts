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

/**
 * A project plus the state of its working tree.
 *
 * A project is a document: `documentPath` is its identity, and `directory`,
 * `fileName` and `kind` are derived from it by the backend.
 */
export type ProjectSummary = {
  id: number;
  name: string;
  documentPath: string;
  directory: string;
  /** The enclosing folder's name, shown above the project's own name. */
  location: string;
  fileName: string;
  kind: DocumentKind;
  engine: Engine;
  /** Kept at the top of the library, ahead of whatever was opened last. */
  pinned: boolean;
  createdAt: number;
  lastOpenedAt: number;
  build: BuildState;
  artifact: ArtifactSummary | null;
  /** Stored versions. The live working tree is not one of them. */
  snapshotCount: number;
  /** The document is still where Press left it. */
  available: boolean;
};

/**
 * What asking for a snapshot led to. Content Press already keeps is turned
 * away rather than stored under a second name, and says which version it
 * already is.
 */
export type SnapshotOutcome =
  | ({ status: 'stored' } & SnapshotSummary)
  | { status: 'unchanged'; title: string };

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

/**
 * The source behind a place in a built PDF. `text` is that file as the version
 * being read has it, so a snapshot answers with the source it was built from.
 */
export type SourcePeek = {
  /** Project-relative. */
  file: string;
  /** 1-based and inclusive. */
  startLine: number;
  endLine: number;
  text: string;
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

export type ToolInfo = {
  available: boolean;
  path: string | null;
  version: string | null;
};

/** One document a path resolved to. `projectId` is set when Press has it already. */
export type OpenCandidate = {
  documentPath: string;
  /** Suggested `<folder>/<file>` name, ignored when the project exists. */
  name: string;
  kind: DocumentKind;
  engine: Engine | null;
  projectId: number | null;
  /** latexmk configuration beside this document; these are executable Perl. */
  latexmkrcPaths: string[];
};

/**
 * What a path resolved to. The Add button, `:Press` and `press <path>` all ask
 * the same question and all get this back.
 */
export type OpenRequest = {
  path: string;
  candidates: OpenCandidate[];
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
