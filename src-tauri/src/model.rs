use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize, Serializer, de::Error as _};

use crate::error::{AppError, AppResult};

/// Which version of a project's source a build refers to.
///
/// Phase 1 only ever builds the working tree. Snapshots are stored as opaque
/// revision tokens so that the cache key, the build queue, the database and the
/// event payloads already carry the dimension that Press's own history needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceRef {
    Worktree,
    Snapshot(String),
}

impl SourceRef {
    pub const WORKTREE_TOKEN: &'static str = "worktree";

    /// A filesystem-safe component used to separate per-version scratch space.
    pub fn slug(&self) -> String {
        match self {
            Self::Worktree => Self::WORKTREE_TOKEN.to_owned(),
            Self::Snapshot(revision) => {
                let safe = revision
                    .chars()
                    .map(|character| {
                        if character.is_ascii_alphanumeric() {
                            character
                        } else {
                            '-'
                        }
                    })
                    .take(64)
                    .collect::<String>();
                format!("snapshot-{safe}")
            }
        }
    }
}

impl fmt::Display for SourceRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worktree => formatter.write_str(Self::WORKTREE_TOKEN),
            Self::Snapshot(revision) => write!(formatter, "snapshot:{revision}"),
        }
    }
}

impl FromStr for SourceRef {
    type Err = AppError;

    fn from_str(value: &str) -> AppResult<Self> {
        if value == Self::WORKTREE_TOKEN {
            return Ok(Self::Worktree);
        }
        match value.strip_prefix("snapshot:") {
            Some(revision) if !revision.is_empty() => Ok(Self::Snapshot(revision.to_owned())),
            _ => Err(AppError::InvalidInput(format!(
                "{value} is not a known source reference"
            ))),
        }
    }
}

impl Serialize for SourceRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SourceRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(D::Error::custom)
    }
}

/// What a project is written in. Both end at latexmk: markdown reaches it
/// through pandoc, which emits LaTeX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentKind {
    Latex,
    Markdown,
}

impl DocumentKind {
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Latex => "latex",
            Self::Markdown => "markdown",
        }
    }

    /// Markdown extensions pandoc reads. Anything else is treated as LaTeX.
    pub fn of(path: &Path) -> Self {
        let markdown = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "md" | "markdown" | "mdown" | "mkd" | "qmd"
                )
            });
        if markdown { Self::Markdown } else { Self::Latex }
    }
}

impl fmt::Display for DocumentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_token())
    }
}

impl FromStr for DocumentKind {
    type Err = AppError;

    fn from_str(value: &str) -> AppResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "latex" | "tex" => Ok(Self::Latex),
            "markdown" | "md" => Ok(Self::Markdown),
            other => Err(AppError::InvalidInput(format!(
                "{other} is not a kind of document Press compiles"
            ))),
        }
    }
}

/// The TeX engine a project compiles with. Recorded per project, never guessed
/// per launch, because two versions built by different engines cannot be compared.
// The shared suffix is the domain's own naming, not an accident.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    PdfLatex,
    XeLatex,
    LuaLatex,
}

impl Engine {
    pub fn latexmk_flag(self) -> &'static str {
        match self {
            Self::PdfLatex => "-pdf",
            Self::XeLatex => "-pdfxe",
            Self::LuaLatex => "-pdflua",
        }
    }

    pub fn as_token(self) -> &'static str {
        match self {
            Self::PdfLatex => "pdflatex",
            Self::XeLatex => "xelatex",
            Self::LuaLatex => "lualatex",
        }
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_token())
    }
}

impl FromStr for Engine {
    type Err = AppError;

    fn from_str(value: &str) -> AppResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pdflatex" | "pdftex" => Ok(Self::PdfLatex),
            "xelatex" | "xetex" => Ok(Self::XeLatex),
            "lualatex" | "luatex" => Ok(Self::LuaLatex),
            other => Err(AppError::InvalidInput(format!(
                "{other} is not a supported TeX engine"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildStatus {
    Never,
    Queued,
    Running,
    Success,
    Error,
    Interrupted,
}

impl BuildStatus {
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Success => "success",
            Self::Error => "error",
            Self::Interrupted => "interrupted",
        }
    }
}

impl fmt::Display for BuildStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_token())
    }
}

impl FromStr for BuildStatus {
    type Err = AppError;

    fn from_str(value: &str) -> AppResult<Self> {
        match value {
            "never" => Ok(Self::Never),
            "queued" => Ok(Self::Queued),
            "running" | "building" => Ok(Self::Running),
            "success" => Ok(Self::Success),
            "error" => Ok(Self::Error),
            "interrupted" => Ok(Self::Interrupted),
            other => Err(AppError::InvalidInput(format!(
                "{other} is not a known build status"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Error,
    Warning,
}

/// One structured problem from a build. Shaped for Neovim's quickfix list, which
/// is where these are headed once the RPC channel exists.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// Project-relative when the path could be resolved inside the project.
    pub file: Option<String>,
    pub line: Option<u32>,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildState {
    pub source_ref: SourceRef,
    pub status: BuildStatus,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    /// The single line shown in the strip above the PDF.
    pub error_summary: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl BuildState {
    pub fn never(source_ref: SourceRef) -> Self {
        Self {
            source_ref,
            status: BuildStatus::Never,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            error_summary: None,
            diagnostics: Vec::new(),
        }
    }
}

/// A published PDF. Immutable for snapshots, replaced in place for the worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactSummary {
    pub id: i64,
    pub project_id: i64,
    pub source_ref: SourceRef,
    pub engine: Engine,
    pub page_count: Option<i64>,
    pub byte_size: i64,
    pub built_at: i64,
    /// Increments whenever this artifact's bytes are replaced, so the webview can
    /// cache-bust without re-reading the PDF.
    pub revision: i64,
}

/// A project as stored.
///
/// A project *is* a document. `document_path` is its identity and its only
/// location data: the directory latexmk runs in, the tree the watcher follows,
/// the files a snapshot holds and the kind of document it is all derive from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub document_path: String,
    pub engine: Engine,
    /// Kept at the top of the library, ahead of whatever was opened last.
    pub pinned: bool,
    pub created_at: i64,
    pub last_opened_at: i64,
}

impl Project {
    pub fn document(&self) -> PathBuf {
        PathBuf::from(&self.document_path)
    }

    /// Where latexmk runs, what the watcher follows, and what a snapshot holds.
    pub fn directory(&self) -> PathBuf {
        self.document()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    /// The folder the project's own folder sits in, which is the line the
    /// library shows above the name.
    ///
    /// For `/Users/a/Projects/ProjectName/thesis/main.tex` that is
    /// `ProjectName`: the name already carries `thesis/main.tex`, so this is
    /// one step further out and no more. Empty when there is no step to take.
    pub fn location(&self) -> String {
        self.document()
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned()
    }

    /// The document's own name, which is the argument latexmk receives.
    pub fn file_name(&self) -> String {
        Path::new(&self.document_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("document")
            .to_owned()
    }

    pub fn kind(&self) -> DocumentKind {
        DocumentKind::of(Path::new(&self.document_path))
    }

    pub fn job_name(&self) -> String {
        Path::new(&self.document_path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or("document")
            .to_owned()
    }
}

/// A project plus everything the library and the viewer need about its working
/// tree. Snapshot state is fetched separately so this payload stays small.
///
/// `kind`, `directory`, `location` and `file_name` are derived from the document
/// path and sent along so the interface never has to take a path apart itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    #[serde(flatten)]
    pub project: Project,
    pub kind: DocumentKind,
    pub directory: String,
    /// The enclosing folder's name, shown above the project's own name.
    pub location: String,
    pub file_name: String,
    pub build: BuildState,
    pub artifact: Option<ArtifactSummary>,
    /// Stored versions. The working tree is live rather than stored, so it is
    /// not counted: this is how many snapshots the library card reports.
    pub snapshot_count: i64,
    /// The document is still where Press left it.
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainReport {
    pub latexmk: ToolInfo,
    pub pandoc: ToolInfo,
    pub neovim: ToolInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorLaunchResult {
    pub status: String,
    pub socket_path: String,
    pub message: String,
}

/// One document a path resolved to.
///
/// `project_id` is set when Press already keeps this document, which is the
/// only difference between opening something and adding it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCandidate {
    pub document_path: String,
    /// Suggested name, `<folder>/<file>`. Ignored when the project exists.
    pub name: String,
    pub kind: DocumentKind,
    pub engine: Option<Engine>,
    pub project_id: Option<i64>,
    /// latexmk configuration beside this document. Executable Perl, so it is
    /// always shown before the document is added.
    pub latexmkrc_paths: Vec<String>,
}

/// What a path from outside resolved to: from the command line, from `:Press`,
/// or from the library's Add button. Every one of them asks the same question,
/// so they all get the same answer.
///
/// One candidate and nothing to warn about means open it. Several means ask.
/// None means say why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRequest {
    pub path: String,
    pub candidates: Vec<OpenCandidate>,
    pub warnings: Vec<String>,
    pub toolchain: ToolchainReport,
}

/// A stored version of a project's source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSummary {
    pub id: i64,
    pub project_id: i64,
    /// Manifest hash. Shared by any two snapshots of identical content, which is
    /// also what lets them share a cached build.
    pub revision: String,
    pub title: String,
    pub body: Option<String>,
    pub created_at: i64,
    pub file_count: i64,
    pub byte_size: i64,
}

/// What asking for a snapshot led to.
///
/// A version that holds nothing new is not a version, so identical content is
/// turned away rather than stored twice. That is not a failure — the source is
/// already kept, under the name this carries — so it is an answer, not an
/// error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SnapshotOutcome {
    Stored {
        #[serde(flatten)]
        snapshot: SnapshotSummary,
    },
    Unchanged {
        /// The version this content already is.
        title: String,
    },
}

impl SnapshotOutcome {
    /// The snapshot that was stored, or `None` when this content was already
    /// kept under another title.
    ///
    /// For tests. Everything else matches on the outcome, so that the case
    /// where nothing was stored has to be answered rather than unwrapped past.
    #[cfg(test)]
    pub fn stored(self) -> Option<SnapshotSummary> {
        match self {
            Self::Stored { snapshot } => Some(snapshot),
            Self::Unchanged { .. } => None,
        }
    }
}

/// One row of the history: the working tree, or a snapshot, together with what
/// Press knows about building it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionSummary {
    pub source_ref: SourceRef,
    pub title: String,
    pub snapshot: Option<SnapshotSummary>,
    pub build: BuildState,
    pub artifact: Option<ArtifactSummary>,
}

/// A page's size in PDF points. The viewer lays the whole document out from
/// these before a single page has been drawn, so scroll position and the page
/// counter are correct immediately.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// One word and where it sits, in PDF points from the page's top left. The
/// selection overlay is built from these.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBox {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Live build progress. Emitted from parsed latexmk output rather than a timer,
/// so the banner can say something true about a long build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildProgress {
    pub build_id: u64,
    pub project_id: i64,
    pub source_ref: SourceRef,
    pub stage: String,
    pub pass: Option<u32>,
    pub page: Option<u32>,
    /// Page count of the previous successful build, used as the denominator.
    pub expected_pages: Option<i64>,
}

/// Emitted whenever a build's state changes, for any version of any project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildUpdate {
    pub build_id: Option<u64>,
    pub project_id: i64,
    pub source_ref: SourceRef,
    pub build: BuildState,
    pub artifact: Option<ArtifactSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_refs_round_trip_through_their_tokens() {
        let worktree: SourceRef = SourceRef::WORKTREE_TOKEN.parse().unwrap();
        assert_eq!(worktree, SourceRef::Worktree);
        let snapshot: SourceRef = "snapshot:abc123".parse().unwrap();
        assert_eq!(snapshot, SourceRef::Snapshot("abc123".into()));
        assert_eq!(snapshot.to_string(), "snapshot:abc123");
        assert!("snapshot:".parse::<SourceRef>().is_err());
        assert!("nonsense".parse::<SourceRef>().is_err());
    }

    #[test]
    fn snapshot_slugs_stay_filesystem_safe() {
        let slug = SourceRef::Snapshot("../../etc/passwd".into()).slug();
        assert!(!slug.contains('/'));
        assert!(!slug.contains('.'));
        assert_eq!(SourceRef::Worktree.slug(), "worktree");
    }

    #[test]
    fn document_kinds_follow_the_file_extension() {
        for name in ["notes.md", "paper.markdown", "report.QMD"] {
            assert_eq!(DocumentKind::of(Path::new(name)), DocumentKind::Markdown, "{name}");
        }
        for name in ["main.tex", "thesis.ltx", "analysis.Rnw"] {
            assert_eq!(DocumentKind::of(Path::new(name)), DocumentKind::Latex, "{name}");
        }
        assert_eq!("md".parse::<DocumentKind>().unwrap(), DocumentKind::Markdown);
        assert!("docx".parse::<DocumentKind>().is_err());
    }

    #[test]
    fn engines_map_to_latexmk_flags() {
        assert_eq!(Engine::PdfLatex.latexmk_flag(), "-pdf");
        assert_eq!(Engine::XeLatex.latexmk_flag(), "-pdfxe");
        assert_eq!(Engine::LuaLatex.latexmk_flag(), "-pdflua");
        assert_eq!("XeTeX".parse::<Engine>().unwrap(), Engine::XeLatex);
        assert!("ptex".parse::<Engine>().is_err());
    }

    /// The webview reads these field names directly. A rename here that is not
    /// mirrored in `src/lib/types.ts` breaks the interface silently, so the
    /// payload shape is pinned.
    #[test]
    fn project_summaries_serialize_flat_for_the_webview() {
        let project = Project {
            id: 3,
            name: "thesis/main.tex".into(),
            document_path: "/projects/thesis/main.tex".into(),
            engine: Engine::LuaLatex,
            pinned: true,
            created_at: 100,
            last_opened_at: 200,
        };
        let summary = ProjectSummary {
            kind: project.kind(),
            directory: "/projects/thesis".into(),
            location: project.location(),
            file_name: project.file_name(),
            project,
            build: BuildState {
                source_ref: SourceRef::Worktree,
                status: BuildStatus::Error,
                started_at: Some(1),
                finished_at: Some(2),
                duration_ms: Some(1000),
                error_summary: Some("main.tex:4: Missing $ inserted.".into()),
                diagnostics: vec![Diagnostic {
                    file: Some("main.tex".into()),
                    line: Some(4),
                    severity: Severity::Error,
                    message: "Missing $ inserted.".into(),
                }],
            },
            artifact: Some(ArtifactSummary {
                id: 9,
                project_id: 3,
                source_ref: SourceRef::Snapshot("abc123".into()),
                engine: Engine::LuaLatex,
                page_count: Some(42),
                byte_size: 1024,
                built_at: 300,
                revision: 5,
            }),
            snapshot_count: 3,
            available: true,
        };

        let json = serde_json::to_value(&summary).unwrap();
        // Project fields are flattened, not nested under `project`.
        assert_eq!(json["id"], 3);
        assert_eq!(json["documentPath"], "/projects/thesis/main.tex");
        assert_eq!(json["directory"], "/projects/thesis");
        // The step above the name, which already carries `thesis/main.tex`.
        assert_eq!(json["location"], "projects");
        assert_eq!(json["fileName"], "main.tex");
        assert_eq!(json["pinned"], true);
        assert_eq!(json["engine"], "lualatex");
        assert_eq!(json["kind"], "latex");
        assert_eq!(json["lastOpenedAt"], 200);
        assert_eq!(json["snapshotCount"], 3);
        assert_eq!(json["available"], true);
        assert!(json.get("project").is_none());
        // Everything a folder used to be stored for is derived now.
        assert!(json.get("rootPath").is_none());
        assert!(json.get("workingDirectory").is_none());

        assert_eq!(json["build"]["status"], "error");
        assert_eq!(json["build"]["sourceRef"], "worktree");
        assert_eq!(json["build"]["durationMs"], 1000);
        assert_eq!(json["build"]["errorSummary"], "main.tex:4: Missing $ inserted.");
        assert_eq!(json["build"]["diagnostics"][0]["severity"], "error");
        assert_eq!(json["build"]["diagnostics"][0]["line"], 4);

        assert_eq!(json["artifact"]["id"], 9);
        assert_eq!(json["artifact"]["sourceRef"], "snapshot:abc123");
        assert_eq!(json["artifact"]["pageCount"], 42);
        assert_eq!(json["artifact"]["byteSize"], 1024);
        assert_eq!(json["artifact"]["revision"], 5);
    }

    #[test]
    fn build_status_tokens_match_the_webview_union() {
        for (status, token) in [
            (BuildStatus::Never, "never"),
            (BuildStatus::Queued, "queued"),
            (BuildStatus::Running, "running"),
            (BuildStatus::Success, "success"),
            (BuildStatus::Error, "error"),
            (BuildStatus::Interrupted, "interrupted"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), token);
            assert_eq!(status.as_token(), token);
            assert_eq!(token.parse::<BuildStatus>().unwrap(), status);
        }
    }

    /// Everything a build needs comes out of the one path it is stored under.
    #[test]
    fn a_project_derives_its_whole_location_from_the_document() {
        let project = Project {
            id: 1,
            name: "papers/main.tex".into(),
            document_path: "/projects/thesis/papers/main.tex".into(),
            engine: Engine::PdfLatex,
            pinned: false,
            created_at: 0,
            last_opened_at: 0,
        };
        assert_eq!(project.directory(), Path::new("/projects/thesis/papers"));
        assert_eq!(project.file_name(), "main.tex");
        assert_eq!(project.job_name(), "main");
        assert_eq!(project.kind(), DocumentKind::Latex);
        // One step beyond the name, which is already `papers/main.tex`.
        assert_eq!(project.location(), "thesis");

        let essay = Project {
            document_path: "/writing/essay.md".into(),
            ..project
        };
        assert_eq!(essay.directory(), Path::new("/writing"));
        assert_eq!(essay.kind(), DocumentKind::Markdown);
        // Nothing above the folder to name.
        assert_eq!(essay.location(), "");
    }
}
