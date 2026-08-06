use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;
use walkdir::{DirEntry, WalkDir};

use crate::{
    error::{AppError, AppResult},
    model::{DiscoveryReport, DocumentKind, Engine, MainCandidate, path_to_string},
    toolchain::detect_toolchain,
};

const MAX_TEX_FILES: usize = 512;
const MAX_SCAN_BYTES: u64 = 256 * 1024;
/// How far into a file the `% !TEX` directives are still honoured.
const DIRECTIVE_LINES: usize = 50;

/// Extensions that can hold a document root. `.Rnw` is knitr, which produces a
/// `.tex` but is what the author actually edits.
const SOURCE_EXTENSIONS: &[&str] = &["tex", "ltx", "rnw"];
/// Markdown pandoc can read.
const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd", "qmd"];

static ROOT_DIRECTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*%\s*!tex\s+root\s*=\s*(.+?)\s*$").unwrap());
static PROGRAM_DIRECTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*%\s*!tex\s+(?:ts-)?program\s*=\s*(.+?)\s*$").unwrap());
static INCLUSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\(?:input|include|subfile|subfileinclude)\s*\{([^}]+)\}").unwrap()
});
/// `\import{dir/}{file}` and `\subimport{dir/}{file}` take their path in two parts.
static IMPORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:sub)?import\s*\{([^}]*)\}\s*\{([^}]+)\}").unwrap());

#[derive(Default)]
struct CandidateEvidence {
    document_class: bool,
    begins_document: bool,
    conventional_name: bool,
    root_level: bool,
    referenced_as_root: bool,
    /// Another file in the project pulls this one in, so it is a part, not a root.
    included_elsewhere: bool,
}

pub fn inspect(path: &Path) -> AppResult<DiscoveryReport> {
    if !path.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    let root = path.canonicalize()?;
    let root_text = path_to_string(&root)
        .ok_or_else(|| AppError::InvalidInput("project path is not valid UTF-8".into()))?;
    let project_name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("LaTeX project")
        .to_owned();

    let mut tex_files = Vec::new();
    let mut latexmkrc_paths = Vec::new();
    let mut scan_error_count = 0;
    let mut truncated = false;
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .max_depth(16)
        // Deterministic order, so a truncated scan truncates the same way twice.
        .sort_by_file_name()
        .into_iter()
        .filter_entry(should_visit)
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                scan_error_count += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name == ".latexmkrc" || name == "latexmkrc" {
            if let Ok(relative) = entry.path().strip_prefix(&root) {
                latexmkrc_paths.push(portable_path(relative));
            }
            continue;
        }
        if is_source_file(entry.path()) {
            tex_files.push(entry.into_path());
            if tex_files.len() == MAX_TEX_FILES {
                truncated = true;
                break;
            }
        }
    }

    let mut evidence: HashMap<PathBuf, CandidateEvidence> = HashMap::new();
    let mut root_references = HashSet::new();
    let mut included = HashSet::new();
    let mut unreadable_tex_count = 0;
    let mut detected_engine = None;

    for file in &tex_files {
        let relative = relative_path(&root, file)?;
        let metadata = match inspect_tex_file(file) {
            Ok(metadata) => metadata,
            Err(_) => {
                unreadable_tex_count += 1;
                continue;
            }
        };
        let parent = file.parent().unwrap_or(&root).to_path_buf();

        let item = evidence.entry(relative.clone()).or_default();
        item.document_class = metadata.has_document_class;
        item.begins_document = metadata.begins_document;
        item.root_level = relative.components().count() == 1;
        item.conventional_name = conventional_name(&relative);

        if let Some(directive) = &metadata.root_directive
            && let Some(resolved) = resolve_inside_root(&root, &parent, directive)
            && resolved.is_file()
            && is_source_file(&resolved)
        {
            root_references.insert(relative_path(&root, &resolved)?);
        }
        if detected_engine.is_none()
            && let Some(program) = &metadata.program
        {
            detected_engine = program.parse::<Engine>().ok();
        }

        // Anything this file pulls in is a part of a document, not a document.
        for target in metadata.inclusions {
            for candidate in inclusion_candidates(&root, &parent, &target) {
                if let Ok(relative) = relative_path(&root, &candidate) {
                    included.insert(relative);
                }
            }
        }
    }

    for referenced in root_references {
        evidence.entry(referenced).or_default().referenced_as_root = true;
    }
    for part in included {
        if let Some(item) = evidence.get_mut(&part) {
            item.included_elsewhere = true;
        }
    }

    let mut candidates = evidence
        .into_iter()
        .filter_map(|(path, evidence)| candidate(path, evidence))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });

    let recommended_main = recommend(&candidates);
    let requires_selection = recommended_main.is_none() && candidates.len() > 1;

    let mut warnings = Vec::new();
    if truncated {
        warnings.push(format!(
            "Stopped after {MAX_TEX_FILES} TeX files; narrow the project folder if the main file is missing."
        ));
    }
    if scan_error_count > 0 {
        warnings.push(format!(
            "Could not inspect {scan_error_count} project path(s); check folder permissions if the main file is missing."
        ));
    }
    if unreadable_tex_count > 0 {
        warnings.push(format!(
            "Could not read {unreadable_tex_count} TeX file(s); check their permissions if the main file is missing."
        ));
    }
    if tex_files.is_empty() {
        // The second sentence matters: a folder of markdown lands here, and
        // without it the failure looks like Press simply cannot read markdown.
        warnings.push(
            "No LaTeX source files were found in this folder. A markdown document is opened \
             from the file itself, not from its folder."
                .into(),
        );
    } else if candidates.is_empty() {
        warnings.push("LaTeX files were found, but none appears to be a document root.".into());
    }

    Ok(DiscoveryReport {
        root_path: root_text,
        project_name,
        tex_file_count: tex_files.len(),
        candidates,
        recommended_main,
        requires_selection,
        latexmkrc_paths,
        detected_engine,
        warnings,
        toolchain: detect_toolchain(),
    })
}

/// A report for one file, named directly.
///
/// This is the whole answer for markdown, which carries none of the evidence
/// LaTeX does — no `\documentclass`, no `\begin{document}`, no inclusion graph —
/// so there is no honest way to look at a folder and say which markdown file is
/// *the* document. The only reliable signal is someone naming the file.
///
/// It works for a named `.tex` too, which is what picking a file rather than a
/// folder means: this document, in this folder, and do not go looking.
pub fn document_report(file: &Path) -> AppResult<DiscoveryReport> {
    let file = file.canonicalize()?;
    if !file.is_file() {
        return Err(AppError::InvalidInput(format!(
            "{} is not a file",
            file.display()
        )));
    }
    if !is_source_file(&file) && !is_markdown_file(&file) {
        return Err(AppError::InvalidInput(
            "Press compiles LaTeX (.tex, .ltx, .Rnw) and markdown (.md) documents.".into(),
        ));
    }
    let kind = DocumentKind::of(&file);
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::InvalidInput("that file has no usable name".into()))?
        .to_owned();
    let root = file
        .parent()
        .ok_or_else(|| AppError::InvalidInput("that file has no folder".into()))?
        .to_path_buf();
    let root_text = path_to_string(&root)
        .ok_or_else(|| AppError::InvalidInput("project path is not valid UTF-8".into()))?;

    // latexmk still runs in this folder, so a configuration file here is still
    // executable Perl and still worth warning about.
    let latexmkrc_paths = [".latexmkrc", "latexmkrc"]
        .into_iter()
        .filter(|candidate| root.join(candidate).is_file())
        .map(ToOwned::to_owned)
        .collect();

    Ok(DiscoveryReport {
        root_path: root_text,
        // Named after the document rather than the folder: several documents can
        // share a folder, and the folder's name would be the same for all of them.
        project_name: file
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Document")
            .to_owned(),
        tex_file_count: usize::from(kind == DocumentKind::Latex),
        candidates: vec![MainCandidate {
            relative_path: name.clone(),
            kind,
            score: 100,
            reasons: vec!["named directly".into()],
        }],
        recommended_main: Some(name),
        requires_selection: false,
        latexmkrc_paths,
        // A markdown document says nothing about the TeX engine.
        detected_engine: match kind {
            DocumentKind::Latex => detect_engine(&file).ok(),
            DocumentKind::Markdown => None,
        },
        warnings: Vec::new(),
        toolchain: detect_toolchain(),
    })
}

pub fn validate_main(root: &Path, relative: &str) -> AppResult<PathBuf> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::InvalidInput(
            "the main file must be inside the selected project".into(),
        ));
    }
    let main = root.join(relative_path).canonicalize()?;
    let root = root.canonicalize()?;
    if !main.starts_with(&root) || !main.is_file() {
        return Err(AppError::InvalidInput(
            "the main file must be an existing file inside the selected project".into(),
        ));
    }
    if !is_source_file(&main) && !is_markdown_file(&main) {
        return Err(AppError::InvalidInput(
            "the main file must be LaTeX (.tex, .ltx, .Rnw) or markdown (.md)".into(),
        ));
    }
    Ok(main)
}

pub fn detect_engine(main: &Path) -> AppResult<Engine> {
    let metadata = inspect_tex_file(main)?;
    Ok(metadata
        .program
        .and_then(|program| program.parse().ok())
        .unwrap_or(Engine::PdfLatex))
}

fn candidate(path: PathBuf, evidence: CandidateEvidence) -> Option<MainCandidate> {
    if !evidence.document_class && !evidence.referenced_as_root {
        return None;
    }
    let mut score = 0;
    let mut reasons = Vec::new();
    if evidence.document_class {
        score += 100;
        reasons.push("contains \\documentclass".into());
    }
    if evidence.begins_document {
        score += 40;
        reasons.push("opens a document body".into());
    }
    if evidence.referenced_as_root {
        score += 60;
        reasons.push("referenced by a % !TEX root directive".into());
    }
    if evidence.conventional_name {
        score += 30;
        reasons.push("has a conventional main-document name".into());
    }
    if evidence.root_level {
        score += 15;
        reasons.push("is at the project root".into());
    }
    if evidence.included_elsewhere {
        // Decisive: standalone figures and subfiles carry their own
        // \documentclass, and this is what separates them from a real root.
        score -= 150;
        reasons.push("is included by another file, so it is part of a document".into());
    }
    Some(MainCandidate {
        relative_path: portable_path(&path),
        kind: DocumentKind::Latex,
        score,
        reasons,
    })
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MARKDOWN_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn recommend(candidates: &[MainCandidate]) -> Option<String> {
    match candidates {
        [] => None,
        [only] if only.score > 0 => Some(only.relative_path.clone()),
        [only] => {
            let _ = only;
            None
        }
        [first, second, ..] if first.score > 0 && first.score >= second.score + 25 => {
            Some(first.relative_path.clone())
        }
        _ => None,
    }
}

fn conventional_name(path: &Path) -> bool {
    path.file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| {
            matches!(
                stem.to_ascii_lowercase().as_str(),
                "main"
                    | "thesis"
                    | "dissertation"
                    | "paper"
                    | "report"
                    | "book"
                    | "article"
                    | "document"
            )
        })
}

fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SOURCE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

struct TexMetadata {
    has_document_class: bool,
    begins_document: bool,
    root_directive: Option<String>,
    program: Option<String>,
    inclusions: Vec<String>,
}

fn inspect_tex_file(path: &Path) -> AppResult<TexMetadata> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_SCAN_BYTES).read_to_end(&mut bytes)?;
    // Lossy so a Latin-1 source still yields its ASCII LaTeX commands.
    let text = String::from_utf8_lossy(&bytes);
    let mut metadata = TexMetadata {
        has_document_class: false,
        begins_document: false,
        root_directive: None,
        program: None,
        inclusions: Vec::new(),
    };

    for (index, line) in text.lines().enumerate() {
        if index < DIRECTIVE_LINES {
            let directive_line = line.trim_start_matches('\u{feff}');
            if metadata.root_directive.is_none() {
                metadata.root_directive = ROOT_DIRECTIVE
                    .captures(directive_line)
                    .and_then(|capture| capture.get(1))
                    .map(|value| trim_quotes(value.as_str().trim()));
            }
            if metadata.program.is_none() {
                metadata.program = PROGRAM_DIRECTIVE
                    .captures(directive_line)
                    .and_then(|capture| capture.get(1))
                    .map(|value| trim_quotes(value.as_str().trim()));
            }
        }

        let active = strip_tex_comment(line);
        let trimmed = active.trim_start_matches(|character: char| {
            character.is_whitespace() || character == '\u{feff}'
        });
        if trimmed.strip_prefix("\\documentclass").is_some_and(|rest| {
            let rest = rest.trim_start();
            rest.starts_with('{') || rest.starts_with('[')
        }) {
            metadata.has_document_class = true;
        }
        if trimmed.starts_with("\\begin{document}") {
            metadata.begins_document = true;
        }
        for capture in INCLUSION.captures_iter(active) {
            if let Some(target) = capture.get(1) {
                metadata.inclusions.push(target.as_str().trim().to_owned());
            }
        }
        for capture in IMPORT.captures_iter(active) {
            if let (Some(directory), Some(file)) = (capture.get(1), capture.get(2)) {
                metadata
                    .inclusions
                    .push(format!("{}{}", directory.as_str().trim(), file.as_str().trim()));
            }
        }
    }
    Ok(metadata)
}

/// An inclusion may omit the extension, and may be written relative to either the
/// including file or the project root. Both readings are recorded, because a
/// false positive only demotes a file that was already unlikely to be the root.
fn inclusion_candidates(root: &Path, parent: &Path, target: &str) -> Vec<PathBuf> {
    let target = target.trim().trim_matches('"');
    if target.is_empty() {
        return Vec::new();
    }
    let mut names = vec![target.to_owned()];
    if Path::new(target).extension().is_none() {
        names.push(format!("{target}.tex"));
    }
    let mut resolved = Vec::new();
    for name in names {
        for base in [parent, root] {
            if let Some(path) = resolve_inside_root(root, base, &name)
                && path.is_file()
            {
                resolved.push(path);
            }
        }
    }
    resolved
}

fn strip_tex_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let mut slashes = 0;
        let mut cursor = index;
        while cursor > 0 && bytes[cursor - 1] == b'\\' {
            slashes += 1;
            cursor -= 1;
        }
        if slashes % 2 == 0 {
            return &line[..index];
        }
    }
    line
}

fn trim_quotes(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn resolve_inside_root(root: &Path, base: &Path, value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let canonical = joined.canonicalize().ok()?;
    canonical.starts_with(root).then_some(canonical)
}

fn relative_path(root: &Path, path: &Path) -> AppResult<PathBuf> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| AppError::InvalidInput("project file escaped the selected folder".into()))
}

fn portable_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn should_visit(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_str(),
        ".git" | ".svn" | ".hg" | "node_modules" | "target" | "build" | "out" | "dist" | ".cache"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn finds_a_unique_main_and_honors_comments() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "chapter.tex",
            "% \\documentclass{book}\nhello",
        );
        write(directory.path(), "main.tex", "\\documentclass{book}\n");

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert_eq!(report.candidates.len(), 1);
    }

    #[test]
    fn root_directive_strengthens_the_referenced_file() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "main.tex", "\\documentclass{book}\n");
        write(
            directory.path(),
            "chapters/one.tex",
            "% !TEX root = ../main.tex\ntext",
        );

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert!(
            report.candidates[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("root directive"))
        );
    }

    #[test]
    fn standalone_figures_do_not_compete_with_the_document() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "main.tex",
            "\\documentclass{article}\n\\begin{document}\n\\input{figures/plot}\n\\input{figures/chart}\n\\end{document}\n",
        );
        // Both figures are complete standalone documents in their own right.
        write(
            directory.path(),
            "figures/plot.tex",
            "\\documentclass{standalone}\n\\begin{document}\\end{document}\n",
        );
        write(
            directory.path(),
            "figures/chart.tex",
            "\\documentclass{standalone}\n\\begin{document}\\end{document}\n",
        );

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert!(!report.requires_selection);
        let plot = report
            .candidates
            .iter()
            .find(|candidate| candidate.relative_path == "figures/plot.tex")
            .expect("the figure is still listed, just demoted");
        assert!(plot.score < 0);
        assert!(
            plot.reasons
                .iter()
                .any(|reason| reason.contains("included by another file"))
        );
    }

    #[test]
    fn include_without_an_extension_still_demotes_the_part() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "thesis.tex",
            "\\documentclass{book}\n\\include{chapters/one}\n",
        );
        write(
            directory.path(),
            "chapters/one.tex",
            "\\documentclass{standalone}\ncontent\n",
        );

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("thesis.tex"));
    }

    #[test]
    fn subimport_paths_are_understood() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "main.tex",
            "\\documentclass{book}\n\\subimport{parts/}{intro}\n",
        );
        write(
            directory.path(),
            "parts/intro.tex",
            "\\documentclass{standalone}\nhello\n",
        );

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
    }

    #[test]
    fn two_genuine_documents_require_a_choice() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "paper.tex",
            "\\documentclass{article}\n\\begin{document}\\end{document}\n",
        );
        write(
            directory.path(),
            "report.tex",
            "\\documentclass{article}\n\\begin{document}\\end{document}\n",
        );

        let report = inspect(directory.path()).unwrap();
        assert!(report.requires_selection);
        assert_eq!(report.recommended_main, None);
        assert_eq!(report.candidates.len(), 2);
    }

    #[test]
    fn reports_every_latexmk_configuration_in_the_folder() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "papers/main.tex", "\\documentclass{book}\n");
        write(directory.path(), ".latexmkrc", "# perl at the root\n");
        write(directory.path(), "papers/.latexmkrc", "# perl beside the main\n");

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.latexmkrc_paths.len(), 2);
        assert!(report.latexmkrc_paths.contains(&".latexmkrc".to_owned()));
        // The one latexmk picks up from its working directory was previously missed.
        assert!(
            report
                .latexmkrc_paths
                .contains(&"papers/.latexmkrc".to_owned())
        );
    }

    #[test]
    fn refuses_main_files_outside_the_project() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(directory.path().join("outside.tex"), "").unwrap();
        assert!(validate_main(&root, "../outside.tex").is_err());
        assert!(validate_main(&root, "/etc/passwd").is_err());
    }

    #[test]
    fn discovers_ascii_latex_commands_in_non_utf8_sources() {
        let directory = tempfile::tempdir().unwrap();
        let mut source = b"\\documentclass{article}\n% latin-1: ".to_vec();
        source.push(0xff);
        std::fs::write(directory.path().join("main.tex"), source).unwrap();

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
    }

    #[test]
    fn detects_bom_and_texshop_engine_directives() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.tex");
        std::fs::write(
            &main,
            "\u{feff}% !TEX TS-program = xelatex\n\\documentclass{article}\n",
        )
        .unwrap();

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert_eq!(report.detected_engine, Some(Engine::XeLatex));
        assert_eq!(detect_engine(&main).unwrap(), Engine::XeLatex);
    }

    #[test]
    fn accepts_alternative_source_extensions() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "analysis.Rnw", "\\documentclass{article}\n");
        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("analysis.Rnw"));
        assert!(validate_main(&directory.path().canonicalize().unwrap(), "analysis.Rnw").is_ok());
    }

    #[test]
    fn an_empty_folder_explains_itself() {
        let directory = tempfile::tempdir().unwrap();
        let report = inspect(directory.path()).unwrap();
        assert!(report.candidates.is_empty());
        assert!(report.recommended_main.is_none());
        assert!(!report.warnings.is_empty());
    }
}
