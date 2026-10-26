//! Turning a path into the documents it means.
//!
//! Every way into Press asks the same question — `:Press` from a buffer, `press
//! <path>` from a shell, the Add button — so they all come here and all get the
//! same answer: a list of documents, each either a project already or one that
//! could be.
//!
//! Two rules do all the work.
//!
//! * **A named file resolves to its document root.** A markdown file is its own
//!   root. A `.tex` resolves through `% !TEX root` or the main file a subfile
//!   names, then through the inclusion graph above it, followed however deep —
//!   so editing `content/chapter.tex` opens the book even when the main file
//!   only includes `content/index.tex`, and a `standalone` figure opens the
//!   document that includes it. A `.tex` nothing includes is its own root if
//!   it can be: when it has neither a `\documentclass` nor plain TeX's `\bye`
//!   it is a fragment, and the nearest document above it is the one meant.
//! * **A directory is never a project, only a place to look for one.** It lists
//!   what is there and lets the user choose. Nothing is ever guessed at.

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
    database::Repository,
    error::{AppError, AppResult},
    files,
    model::{DocumentKind, Engine, OpenCandidate, OpenRequest, ToolchainReport},
    toolchain::detect_toolchain,
};

/// How deep a directory scan goes, and how many documents it will list before it
/// stops. A folder holding more than this is not one you pick a document from.
const MAX_DEPTH: usize = 16;
const MAX_CANDIDATES: usize = 64;
const MAX_SCAN_BYTES: u64 = 256 * 1024;
/// How far into a file the `% !TEX` directives are still honoured.
const DIRECTIVE_LINES: usize = 50;
/// How far above a named file to look for the document that includes it.
const MAX_ASCENT: usize = 4;
/// How many files one document's inclusions are followed through before the
/// search gives up on it. A book is a few dozen.
const MAX_REACH: usize = 512;

/// Extensions that can hold a document root. `.Rnw` is knitr, which produces a
/// `.tex` but is what the author actually edits.
const LATEX_EXTENSIONS: &[&str] = &["tex", "ltx", "rnw"];
/// Markdown pandoc reads.
const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd", "qmd"];

/// Markdown that documents a folder rather than being a document in it. Only
/// ever applied to a directory listing: naming one of these directly still
/// compiles it, because naming a file is always the last word.
const NOT_DOCUMENTS: &[&str] = &["readme", "changelog", "contributing", "license", "authors"];

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
/// The class, and its options, on a `\documentclass` line.
static DOCUMENT_CLASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\documentclass\s*(?:\[([^\]]*)\])?\s*\{\s*([^}\s]+)\s*\}").unwrap()
});
/// How plain TeX ends a document, and what tells one from a LaTeX fragment.
static PLAIN_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\bye\b").unwrap());

// -- resolving ------------------------------------------------------------

/// Resolves a path to the documents it means.
pub fn resolve(path: &Path, repository: &Repository) -> AppResult<OpenRequest> {
    let canonical = path.canonicalize().map_err(|error| {
        AppError::NotFound(format!("{} cannot be opened: {error}", path.display()))
    })?;
    let display = canonical.to_string_lossy().into_owned();
    let known = repository.project_documents()?;

    // A PDF is not a document Press compiles, but it is one it can show. It
    // stops here rather than becoming a project: there is no source behind it,
    // so there would be nothing to build, snapshot or watch for edits.
    if has_extension(&canonical, &["pdf"]) {
        return Ok(OpenRequest {
            path: display.clone(),
            candidates: Vec::new(),
            pdf: Some(display),
            warnings: Vec::new(),
            toolchain: detect_toolchain(),
            sync: None,
        });
    }

    let (documents, mut warnings) = if canonical.is_dir() {
        in_directory(&canonical, &known)
    } else {
        match document_root(&canonical)? {
            Some(document) => (vec![document], Vec::new()),
            None => (
                Vec::new(),
                vec![format!(
                    "Press compiles LaTeX ({}) and markdown ({}) documents.",
                    LATEX_EXTENSIONS.join(", "),
                    MARKDOWN_EXTENSIONS.join(", ")
                )],
            ),
        }
    };

    if documents.is_empty() && warnings.is_empty() {
        warnings.push(format!(
            "No document was found in {}. Press compiles LaTeX and markdown files.",
            canonical.display()
        ));
    }

    let candidates = documents
        .into_iter()
        .map(|document| describe(&document, &known))
        .collect::<Vec<_>>();

    Ok(OpenRequest {
        path: display,
        candidates,
        pdf: None,
        warnings,
        toolchain: detect_toolchain(),
        sync: None,
    })
}

/// The one place a document's presentation is decided, so the library, the
/// picker and `add_project` all agree.
fn describe(document: &Path, known: &HashMap<PathBuf, i64>) -> OpenCandidate {
    let kind = DocumentKind::of(document);
    OpenCandidate {
        document_path: document.to_string_lossy().into_owned(),
        name: suggested_name(document),
        kind,
        // A markdown document says nothing about the TeX engine; pandoc's
        // output compiles with any of them.
        engine: match kind {
            DocumentKind::Latex => detect_engine(document),
            DocumentKind::Markdown => None,
        },
        project_id: known.get(document).copied(),
        latexmkrc_paths: latexmkrc_beside(document),
    }
}

/// `thesis/main.tex`. The folder alone repeats across a directory of documents
/// and the file alone is a shelf of `main.tex`; together they read. The library
/// shows the folder *above* this one on its own line, so there is no repetition.
/// Renaming is a user action, so this has to be a good default and no more.
pub fn suggested_name(document: &Path) -> String {
    let file = document
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document");
    match document
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
    {
        Some(folder) => format!("{folder}/{file}"),
        None => file.to_owned(),
    }
}

/// The document root a named file belongs to.
///
/// `Ok(None)` means the file is not something Press compiles at all.
pub fn document_root(file: &Path) -> AppResult<Option<PathBuf>> {
    let file = file.canonicalize()?;
    if !file.is_file() {
        return Err(AppError::InvalidInput(format!(
            "{} is not a file",
            file.display()
        )));
    }
    if is_markdown(&file) {
        // Markdown carries no evidence of being part of another document, and
        // inventing some was exactly what made this unreliable.
        return Ok(Some(file));
    }
    if !is_latex(&file) {
        return Ok(None);
    }

    let directory = file.parent().unwrap_or(&file).to_path_buf();
    let metadata = read_latex(&file)?;
    // An explicit directive is the author saying it outright.
    if let Some(directive) = &metadata.root_directive
        && let Some(root) = resolve_relative(&directory, directive)
        && root.is_file()
        && is_latex(&root)
    {
        return Ok(Some(root));
    }

    // The subfiles package says it in the class line: the option is the main
    // file, relative to this one.
    if let Some(main) = &metadata.subfiles_main
        && let Some(root) = inclusion_targets(&directory, &directory, main)
            .into_iter()
            .find(|root| is_latex(root))
    {
        return Ok(Some(root));
    }

    // Otherwise: whichever document in this folder, or the folders above it,
    // pulls this file in, directly or through the files it includes. A
    // repository boundary is as far as one paper reaches.
    let levels = levels_above(&directory);
    for search in &levels {
        if let Some(includer) = includer_of(&file, search) {
            return Ok(Some(includer));
        }
    }

    // Nothing includes it. A file with a `\documentclass` is a document, and
    // so is plain TeX, which has none but ends with `\bye`. Anything else is a
    // fragment that cannot compile on its own — a chapter being drafted, a
    // talk swapped out of the main file — and the document above it is the one
    // editing it is for.
    if !metadata.has_document_class
        && !metadata.plain_tex
        && let Some(document) = levels.iter().find_map(|search| nearest_document(&file, search))
    {
        return Ok(Some(document));
    }
    Ok(Some(file))
}

/// The folder a file is in and the ones above it, as far as a search for its
/// document goes: `MAX_ASCENT` levels, or the top of a repository.
fn levels_above(directory: &Path) -> Vec<PathBuf> {
    let mut levels = Vec::new();
    let mut search = directory.to_path_buf();
    for _ in 0..=MAX_ASCENT {
        levels.push(search.clone());
        if search.join(".git").exists() {
            break;
        }
        match search.parent() {
            Some(parent) if parent != search => search = parent.to_path_buf(),
            _ => break,
        }
    }
    levels
}

/// The document in `directory` that pulls `file` in, if there is one.
fn includer_of(file: &Path, directory: &Path) -> Option<PathBuf> {
    latex_files(directory)
        .into_iter()
        .filter(|candidate| candidate != file)
        .filter(|candidate| read_latex(candidate).is_ok_and(|metadata| metadata.has_document_class))
        .find(|candidate| reaches(candidate, file))
}

/// Whether `file` is part of the document `root`: included by it, or by a
/// file it includes, however deep.
///
/// Every path is read the way TeX reads it — from the folder the document is
/// compiled in, which is the root's own, whichever file the `\input` is in —
/// and, failing that, from the including file's folder, which is how
/// `\subimport` and a good many authors write them.
fn reaches(root: &Path, file: &Path) -> bool {
    let Some(base) = root.parent() else {
        return false;
    };
    let mut seen = HashSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(current) = pending.pop() {
        if !seen.insert(current.clone()) {
            continue;
        }
        if seen.len() > MAX_REACH {
            return false;
        }
        let Ok(metadata) = read_latex(&current) else {
            continue;
        };
        let parent = current.parent().unwrap_or(base);
        for target in &metadata.inclusions {
            for resolved in inclusion_targets(parent, base, target) {
                if resolved == file {
                    return true;
                }
                if is_latex(&resolved) {
                    pending.push(resolved);
                }
            }
        }
    }
    false
}

/// The document a fragment in or below `directory` most likely belongs to:
/// `main.tex` when there is one, otherwise the document most recently saved,
/// which is the one being worked on.
///
/// Never a part that has a class of its own — a `standalone` figure or a
/// `subfiles` chapter — because those are pieces of a document, not what a
/// fragment beside them is for.
fn nearest_document(file: &Path, directory: &Path) -> Option<PathBuf> {
    let documents = latex_files(directory)
        .into_iter()
        .filter(|candidate| candidate != file)
        .filter(|candidate| {
            read_latex(candidate).is_ok_and(|metadata| {
                metadata.has_document_class
                    && !matches!(
                        metadata.document_class.as_deref(),
                        Some("standalone" | "subfiles")
                    )
            })
        })
        .collect::<Vec<_>>();
    let named_main = documents.iter().find(|candidate| {
        candidate
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.eq_ignore_ascii_case("main"))
    });
    named_main.cloned().or_else(|| {
        documents
            .into_iter()
            .max_by_key(|candidate| std::fs::metadata(candidate).and_then(|data| data.modified()).ok())
    })
}

/// Every document root in a directory, known projects first.
///
/// LaTeX roots are the files carrying a `\documentclass` that nothing else in
/// the tree includes. Markdown has no such evidence, so every markdown file is
/// listed — listing is not guessing, and the user still chooses.
fn in_directory(directory: &Path, known: &HashMap<PathBuf, i64>) -> (Vec<PathBuf>, Vec<String>) {
    let mut latex = Vec::new();
    let mut markdown = Vec::new();
    let mut truncated = false;

    for entry in WalkDir::new(directory)
        .follow_links(false)
        .max_depth(MAX_DEPTH)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(should_visit)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.into_path();
        if is_latex(&path) {
            latex.push(path);
        } else if is_markdown(&path) && !documents_the_folder(&path) {
            markdown.push(path);
        }
        if latex.len() + markdown.len() > MAX_CANDIDATES * 8 {
            truncated = true;
            break;
        }
    }

    let mut included = HashSet::new();
    let mut roots = Vec::new();
    for file in &latex {
        let Ok(metadata) = read_latex(file) else {
            continue;
        };
        let parent = file.parent().unwrap_or(directory);
        for target in &metadata.inclusions {
            for resolved in inclusion_targets(parent, directory, target) {
                included.insert(resolved);
            }
        }
        // A `% !TEX root` says this file is a part and names the whole.
        if let Some(directive) = &metadata.root_directive
            && let Some(root) = resolve_relative(parent, directive)
            && root.is_file()
        {
            included.insert(file.clone());
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        if metadata.has_document_class {
            roots.push(file.clone());
        }
    }
    roots.retain(|root| !included.contains(root));
    roots.extend(markdown);
    roots.dedup();

    // Documents Press already keeps come first: from a directory, what you
    // usually mean is something you have opened before.
    roots.sort_by_key(|root| (!known.contains_key(root), root.clone()));

    let mut warnings = Vec::new();
    if truncated || roots.len() > MAX_CANDIDATES {
        roots.truncate(MAX_CANDIDATES);
        warnings.push(format!(
            "More than {MAX_CANDIDATES} documents are here; open the one you want by naming its file."
        ));
    }
    (roots, warnings)
}

/// latexmk reads a configuration from the directory it runs in, which is the
/// document's own. It is executable Perl, so it is always reported.
fn latexmkrc_beside(document: &Path) -> Vec<String> {
    let Some(directory) = document.parent() else {
        return Vec::new();
    };
    [".latexmkrc", "latexmkrc"]
        .into_iter()
        .filter(|name| directory.join(name).is_file())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn detect_engine(document: &Path) -> Option<Engine> {
    read_latex(document).ok()?.program?.parse().ok()
}

/// Checks that a path is still a document Press can compile, which is all
/// `add_project` and `open_project` need to know.
pub fn validate(document: &Path) -> AppResult<PathBuf> {
    if document
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(AppError::InvalidInput(
            "a document is named by its full path".into(),
        ));
    }
    let document = document.canonicalize().map_err(|error| {
        AppError::NotFound(format!("{} cannot be opened: {error}", document.display()))
    })?;
    if !document.is_file() {
        return Err(AppError::NotFound(format!(
            "{} is no longer a file",
            document.display()
        )));
    }
    if !is_latex(&document) && !is_markdown(&document) {
        return Err(AppError::InvalidInput(
            "Press compiles LaTeX (.tex, .ltx, .Rnw) and markdown (.md) documents.".into(),
        ));
    }
    Ok(document)
}

pub fn toolchain() -> ToolchainReport {
    detect_toolchain()
}

// -- reading LaTeX --------------------------------------------------------

struct LatexMetadata {
    has_document_class: bool,
    /// `article`, `book`, `standalone`…
    document_class: Option<String>,
    /// The main file a `subfiles` chapter names in its class options.
    subfiles_main: Option<String>,
    /// Ends the way a plain TeX document does, so it compiles without a
    /// `\documentclass`.
    plain_tex: bool,
    root_directive: Option<String>,
    program: Option<String>,
    inclusions: Vec<String>,
}

fn read_latex(path: &Path) -> AppResult<LatexMetadata> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_SCAN_BYTES).read_to_end(&mut bytes)?;
    // Lossy so a Latin-1 source still yields its ASCII LaTeX commands.
    let text = String::from_utf8_lossy(&bytes);
    let mut metadata = LatexMetadata {
        has_document_class: false,
        document_class: None,
        subfiles_main: None,
        plain_tex: false,
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

        let active = strip_comment(line);
        let trimmed = active.trim_start_matches(|character: char| {
            character.is_whitespace() || character == '\u{feff}'
        });
        if trimmed.strip_prefix("\\documentclass").is_some_and(|rest| {
            let rest = rest.trim_start();
            rest.starts_with('{') || rest.starts_with('[')
        }) {
            metadata.has_document_class = true;
            if metadata.document_class.is_none()
                && let Some(capture) = DOCUMENT_CLASS.captures(trimmed)
            {
                let class = capture.get(2).map(|class| class.as_str().to_owned());
                if class.as_deref() == Some("subfiles") {
                    metadata.subfiles_main = capture
                        .get(1)
                        .map(|option| trim_quotes(option.as_str().trim()))
                        .filter(|option| !option.is_empty());
                }
                metadata.document_class = class;
            }
        }
        if PLAIN_END.is_match(active) {
            metadata.plain_tex = true;
        }
        for capture in INCLUSION.captures_iter(active) {
            if let Some(target) = capture.get(1) {
                metadata.inclusions.push(target.as_str().trim().to_owned());
            }
        }
        for capture in IMPORT.captures_iter(active) {
            if let (Some(directory), Some(file)) = (capture.get(1), capture.get(2)) {
                metadata.inclusions.push(format!(
                    "{}{}",
                    directory.as_str().trim(),
                    file.as_str().trim()
                ));
            }
        }
    }
    Ok(metadata)
}

/// An inclusion may omit the `.tex` and may be written relative to the including
/// file or to the folder the build runs in. Both readings are tried.
fn inclusion_targets(parent: &Path, directory: &Path, target: &str) -> Vec<PathBuf> {
    let target = target.trim().trim_matches('"');
    if target.is_empty() {
        return Vec::new();
    }
    let mut names = vec![target.to_owned()];
    if Path::new(target).extension().is_none() {
        names.push(format!("{target}.tex"));
    }
    let mut resolved = Vec::new();
    for name in &names {
        for base in [parent, directory] {
            if let Some(path) = resolve_relative(base, name)
                && path.is_file()
            {
                resolved.push(path);
            }
        }
    }
    resolved
}

fn latex_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_latex(path))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn resolve_relative(base: &Path, value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    joined.canonicalize().ok()
}

fn strip_comment(line: &str) -> &str {
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

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension.to_ascii_lowercase().as_str()))
}

pub fn is_latex(path: &Path) -> bool {
    has_extension(path, LATEX_EXTENSIONS)
}

pub fn is_markdown(path: &Path) -> bool {
    has_extension(path, MARKDOWN_EXTENSIONS)
}

fn documents_the_folder(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| NOT_DOCUMENTS.contains(&stem.to_ascii_lowercase().as_str()))
}

fn should_visit(entry: &DirEntry) -> bool {
    entry.depth() == 0
        || !entry.file_type().is_dir()
        || !files::is_ignored_directory(&entry.file_name().to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::NewProject, model::Engine};

    fn repository(directory: &Path) -> Repository {
        Repository::open(&directory.join("press.db")).unwrap()
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn paths(request: &OpenRequest) -> Vec<String> {
        request
            .candidates
            .iter()
            .map(|candidate| {
                Path::new(&candidate.document_path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    // -- a named file ----------------------------------------------------

    /// The case that started all this: two markdown files in one folder are two
    /// projects, and opening the second must not hand back the first.
    #[test]
    fn markdown_neighbours_are_separate_documents() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("writing");
        write(&root, "test1.md", "# One\n");
        write(&root, "test2.md", "# Two\n");
        let repository = repository(directory.path());

        let first = resolve(&root.join("test1.md"), &repository).unwrap();
        assert_eq!(paths(&first), ["test1.md"]);
        let project = repository
            .upsert_project(NewProject {
                name: &first.candidates[0].name,
                document_path: &first.candidates[0].document_path,
                engine: Engine::PdfLatex,
            })
            .unwrap();
        assert_eq!(project.name, "writing/test1.md");

        // The neighbour is its own document, not the project just added.
        let second = resolve(&root.join("test2.md"), &repository).unwrap();
        assert_eq!(paths(&second), ["test2.md"]);
        assert_eq!(second.candidates[0].project_id, None);

        // And re-opening the first finds the project rather than offering it again.
        let again = resolve(&root.join("test1.md"), &repository).unwrap();
        assert_eq!(again.candidates[0].project_id, Some(project.id));
    }

    /// The same rule, for LaTeX: supplementary material beside a paper is a
    /// separate document as long as the paper does not include it.
    #[test]
    fn latex_neighbours_are_separate_documents() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "paper.tex", "\\documentclass{article}\n");
        write(&root, "supplementary.tex", "\\documentclass{article}\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("supplementary.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["supplementary.tex"]);
    }

    #[test]
    fn a_chapter_resolves_to_the_document_that_includes_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(
            &root,
            "main.tex",
            "\\documentclass{book}\n\\include{chapters/three}\n",
        );
        write(&root, "chapters/three.tex", "the third chapter\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("chapters/three.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["main.tex"]);
    }

    /// A standalone figure carries its own `\documentclass`, so only the
    /// inclusion graph separates it from a real document.
    #[test]
    fn an_included_figure_opens_the_document_that_includes_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(
            &root,
            "main.tex",
            "\\documentclass{article}\n\\input{figures/plot}\n",
        );
        write(
            &root,
            "figures/plot.tex",
            "\\documentclass{standalone}\n\\begin{document}\\end{document}\n",
        );
        let repository = repository(directory.path());

        let request = resolve(&root.join("figures/plot.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["main.tex"]);
    }

    /// The layout most books and long papers have: the main file pulls in one
    /// file that pulls in the chapters, with every path written from the main
    /// file's folder, because that is where TeX runs and how it reads them.
    #[test]
    fn a_chapter_two_inclusions_deep_resolves_to_the_main_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("book");
        write(
            &root,
            "main.tex",
            "\\documentclass{book}\n\\begin{document}\n\\input{content/content}\n\\end{document}\n",
        );
        write(&root, "content/content.tex", "\\input{content/one}\n\\input{content/two}\n");
        write(&root, "content/one.tex", "\\chapter{One}\n");
        write(&root, "content/two.tex", "\\chapter{Two}\n\\input{content/parts/aside}\n");
        // Written relative to the file that includes it, which some people do.
        write(&root, "content/parts/aside.tex", "\\input{note}\n");
        write(&root, "content/parts/note.tex", "a note\n");
        let repository = repository(directory.path());

        for chapter in [
            "content/content.tex",
            "content/one.tex",
            "content/two.tex",
            "content/parts/aside.tex",
            "content/parts/note.tex",
        ] {
            let request = resolve(&root.join(chapter), &repository).unwrap();
            assert_eq!(paths(&request), ["main.tex"], "{chapter}");
        }
    }

    /// A figure deep in the tree is part of the document that eventually
    /// includes it, even when it has a `\documentclass` of its own.
    #[test]
    fn a_standalone_figure_included_by_a_chapter_opens_the_main_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "main.tex", "\\documentclass{article}\n\\input{content/index}\n");
        write(&root, "content/index.tex", "text\n\\input{figures/tree}%\n");
        write(
            &root,
            "figures/tree.tex",
            "\\documentclass{standalone}\n\\begin{document}\\end{document}\n",
        );
        let repository = repository(directory.path());

        let request = resolve(&root.join("figures/tree.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["main.tex"]);
    }

    /// A fragment nothing includes right now — a talk swapped out of the main
    /// file, a chapter being drafted — cannot compile on its own: it has no
    /// `\documentclass`. The document above it is what editing it is for.
    #[test]
    fn a_fragment_nothing_includes_opens_the_document_above_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("talks");
        write(&root, "main.tex", "\\documentclass{beamer}\n\\input{2026/index}\n");
        write(&root, "2026/index.tex", "\\begin{frame}new\\end{frame}\n");
        write(&root, "2025/index.tex", "\\input{2025/one}\n");
        write(&root, "2025/one.tex", "\\begin{frame}old\\end{frame}\n");
        // Parts with a class of their own are never what a fragment means.
        write(
            &root,
            "2025/diagram.tex",
            "\\documentclass{standalone}\n\\begin{document}\\end{document}\n",
        );
        let repository = repository(directory.path());

        for fragment in ["2025/index.tex", "2025/one.tex"] {
            let request = resolve(&root.join(fragment), &repository).unwrap();
            assert_eq!(paths(&request), ["main.tex"], "{fragment}");
        }
    }

    /// Several documents above a fragment: `main.tex` is the one meant.
    #[test]
    fn a_fragment_prefers_the_main_file_among_several_documents() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "appendix.tex", "\\documentclass{article}\n");
        write(&root, "main.tex", "\\documentclass{article}\n");
        write(&root, "sections/draft.tex", "\\section{Draft}\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("sections/draft.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["main.tex"]);
    }

    /// The subfiles package names the main file in the part's own class line.
    #[test]
    fn a_subfile_names_its_main_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("thesis");
        write(&root, "thesis.tex", "\\documentclass{book}\n");
        write(
            &root,
            "chapters/one.tex",
            "\\documentclass[../thesis]{subfiles}\n\\begin{document}One\\end{document}\n",
        );
        let repository = repository(directory.path());

        let request = resolve(&root.join("chapters/one.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["thesis.tex"]);
    }

    /// Files that include each other are followed once, not forever.
    #[test]
    fn an_inclusion_cycle_still_resolves() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "main.tex", "\\documentclass{article}\n\\input{a}\n");
        write(&root, "a.tex", "\\input{b}\n");
        write(&root, "b.tex", "\\input{a}\n");
        write(&root, "loop/c.tex", "\\input{loop/c}\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("b.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["main.tex"]);
        let request = resolve(&root.join("loop/c.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["main.tex"], "an orphan that includes itself");
    }

    #[test]
    fn a_tex_root_directive_wins_outright() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "thesis.tex", "\\documentclass{book}\n");
        write(
            &root,
            "parts/one.tex",
            "% !TEX root = ../thesis.tex\ntext\n",
        );
        let repository = repository(directory.path());

        let request = resolve(&root.join("parts/one.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["thesis.tex"]);
    }

    #[test]
    fn a_file_nothing_includes_is_its_own_document() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("notes");
        // Plain TeX: no \documentclass at all, and still a document.
        write(&root, "note.tex", "\\hello\n\\bye\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("note.tex"), &repository).unwrap();
        assert_eq!(paths(&request), ["note.tex"]);
    }

    #[test]
    fn a_file_press_does_not_compile_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        write(&root, "results.csv", "a,b\n1,2\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("results.csv"), &repository).unwrap();
        assert!(request.candidates.is_empty());
        assert!(
            request.warnings[0].contains("LaTeX"),
            "{:?}",
            request.warnings
        );
    }

    // -- a directory -----------------------------------------------------

    #[test]
    fn a_directory_lists_its_documents_without_choosing_one() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("writing");
        write(&root, "essay.md", "# An essay\n");
        write(&root, "talk.md", "# A talk\n");
        // Neither of these is a document you meant to compile.
        write(&root, "README.md", "how to build this\n");
        let repository = repository(directory.path());

        let request = resolve(&root, &repository).unwrap();
        assert_eq!(paths(&request), ["essay.md", "talk.md"]);
        assert!(request.candidates.iter().all(|c| c.project_id.is_none()));
    }

    #[test]
    fn a_directory_lists_latex_roots_and_not_their_parts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(
            &root,
            "main.tex",
            "\\documentclass{article}\n\\input{figures/plot}\n\\include{chapters/one}\n",
        );
        write(&root, "chapters/one.tex", "text\n");
        write(
            &root,
            "figures/plot.tex",
            "\\documentclass{standalone}\n\\begin{document}\\end{document}\n",
        );
        write(&root, "poster.tex", "\\documentclass{article}\n");
        let repository = repository(directory.path());

        let request = resolve(&root, &repository).unwrap();
        assert_eq!(paths(&request), ["main.tex", "poster.tex"]);
    }

    #[test]
    fn a_directory_puts_the_projects_it_already_knows_first() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("writing");
        write(&root, "aaa.md", "# First alphabetically\n");
        write(&root, "zzz.md", "# Last alphabetically\n");
        let repository = repository(directory.path());
        let known = root.join("zzz.md").canonicalize().unwrap();
        repository
            .upsert_project(NewProject {
                name: "writing/zzz.md",
                document_path: known.to_str().unwrap(),
                engine: Engine::PdfLatex,
            })
            .unwrap();

        let request = resolve(&root, &repository).unwrap();
        assert_eq!(paths(&request), ["zzz.md", "aaa.md"]);
        assert!(request.candidates[0].project_id.is_some());
    }

    #[test]
    fn an_empty_directory_explains_itself() {
        let directory = tempfile::tempdir().unwrap();
        let repository = repository(directory.path());
        let empty = directory.path().join("nothing");
        std::fs::create_dir(&empty).unwrap();

        let request = resolve(&empty, &repository).unwrap();
        assert!(request.candidates.is_empty());
        assert!(request.warnings[0].contains("No document"));
    }

    #[test]
    fn a_path_that_does_not_exist_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        let repository = repository(directory.path());
        assert!(resolve(&directory.path().join("absent.tex"), &repository).is_err());
    }

    // -- what a candidate carries ----------------------------------------

    #[test]
    fn a_candidate_is_named_after_its_folder_and_file() {
        assert_eq!(
            suggested_name(Path::new("/projects/thesis/main.tex")),
            "thesis/main.tex"
        );
        // At the filesystem root there is no folder name to prefix with.
        assert_eq!(suggested_name(Path::new("/essay.md")), "essay.md");
    }

    #[test]
    fn latexmk_configuration_beside_a_document_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "main.tex", "\\documentclass{article}\n");
        write(&root, ".latexmkrc", "# executable perl\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("main.tex"), &repository).unwrap();
        assert_eq!(request.candidates[0].latexmkrc_paths, [".latexmkrc"]);
    }

    #[test]
    fn the_engine_comes_from_the_documents_own_directive() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(
            &root,
            "main.tex",
            "\u{feff}% !TEX TS-program = xelatex\n\\documentclass{article}\n",
        );
        write(&root, "essay.md", "# An essay\n");
        let repository = repository(directory.path());

        let latex = resolve(&root.join("main.tex"), &repository).unwrap();
        assert_eq!(latex.candidates[0].engine, Some(Engine::XeLatex));
        // Markdown says nothing about the engine, and is not asked.
        let markdown = resolve(&root.join("essay.md"), &repository).unwrap();
        assert_eq!(markdown.candidates[0].engine, None);
        assert_eq!(markdown.candidates[0].kind, DocumentKind::Markdown);
    }

    #[test]
    fn discovery_reads_ascii_latex_out_of_a_non_utf8_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        std::fs::create_dir_all(&root).unwrap();
        let mut source = b"\\documentclass{article}\n% latin-1: ".to_vec();
        source.push(0xff);
        std::fs::write(root.join("main.tex"), source).unwrap();
        let repository = repository(directory.path());

        let request = resolve(&root, &repository).unwrap();
        assert_eq!(paths(&request), ["main.tex"]);
    }

    #[test]
    fn validate_refuses_anything_that_is_not_a_document() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "main.tex", "\\documentclass{article}\n");
        write(&root, "notes.txt", "not a document\n");

        assert!(validate(&root.join("main.tex")).is_ok());
        assert!(validate(&root.join("notes.txt")).is_err());
        assert!(validate(&root).is_err(), "a directory is not a document");
        assert!(validate(&root.join("absent.tex")).is_err());
        assert!(validate(Path::new("/etc/../etc/passwd")).is_err());
    }
}
