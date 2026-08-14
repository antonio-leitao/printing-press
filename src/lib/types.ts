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
 * A link on a page: a rectangle in PDF points and where it leads. Exactly one
 * of `page` and `uri` is set.
 */
export type LinkBox = {
  x: number;
  y: number;
  width: number;
  height: number;
  /** 1-based, for a link into this document. */
  page: number | null;
  /** How far down that page, in PDF points from its top. */
  top: number | null;
  uri: string | null;
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

/** Whether a tool is installed, and where. Nothing asks it for its version. */
export type ToolInfo = {
  available: boolean;
  path: string | null;
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
 * A PDF Press is showing without owning. It never reaches the library: a
 * project is a source document, and this has no source behind it.
 */
export type LooseDocument = {
  /** Negative, so it stands in for an artifact id wherever a page is asked for. */
  id: number;
  name: string;
  path: string;
  /** Bumped when the file changes on disk. */
  revision: number;
};

/**
 * What a path resolved to. The Add button, `:Press` and `press <path>` all ask
 * the same question and all get this back.
 */
export type OpenRequest = {
  path: string;
  candidates: OpenCandidate[];
  /** Set when the path was a PDF, which Press shows rather than compiles. */
  pdf: string | null;
  warnings: string[];
  toolchain: {
    latexmk: ToolInfo;
    pandoc: ToolInfo;
    neovim: ToolInfo;
  };
};

/**
 * The command the Editor button runs. Press spawns it and forgets it: the
 * working tree is watched, so a save rebuilds the document whoever wrote it,
 * and there is no connection to the editor to keep or to lose.
 */
export type EditorCommand = {
  /** What will run — the stored command, or the system's own default. */
  command: string;
  /** A command that suits this machine, offered on a first run. */
  suggested: string;
};

/**
 * Which tile Press wears in the Dock.
 *
 * The names are the ones `make-icons.py` writes and `appearance.rs` compiles
 * in, so all three lists are the same three words or none of this works.
 */
export type IconChoice = 'green' | 'ink' | 'sheet';

/** The three, in the order the settings dialog offers them. */
export const ICON_CHOICES: readonly IconChoice[] = ['green', 'ink', 'sheet'];

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
