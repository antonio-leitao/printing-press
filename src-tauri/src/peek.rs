//! Reading a place in the PDF back to the source that made it.
//!
//! A click carries a page and a point in PDF coordinates. SyncTeX answers with
//! a file and a line, and the rest of the work is turning that into something
//! worth showing: the right file, in the right version, widened to a whole
//! construct rather than the one line a box happened to start on.
//!
//! Two things make this work for a version that is no longer on disk. The sync
//! data is published beside the built PDF rather than left in the scratch
//! directory, so it always describes the PDF being looked at; and a snapshot's
//! source is read out of the object store, so the temporary checkout the build
//! ran in — long since gone — is never needed. A frozen version is the case
//! this handles best: its source cannot have moved since it was built, which is
//! more than the working tree can promise.

use std::path::{Path, PathBuf};

use crate::{
    anchors,
    database::{Repository, StoredArtifact},
    error::{AppError, AppResult},
    model::{DocumentKind, Project, SourcePeek, SourceRef},
    snapshot,
    toolchain::{augmented_path, resolve_executable},
};

/// Lines either side of a paragraph before Press stops widening. A peek is a
/// look at one thing, not a way to read the document.
const MOST_LINES: usize = 80;

/// Where a click landed, in the source that produced it.
pub fn resolve(
    project: &Project,
    stored: &StoredArtifact,
    repository: &Repository,
    objects: &Path,
    page: u32,
    x: f64,
    y: f64,
) -> AppResult<Option<SourcePeek>> {
    let Some(hit) = ask_synctex(&stored.pdf_path, page, x, y)? else {
        return Ok(None);
    };

    let kind = project.kind();
    let generated_name = format!("{}.tex", project.job_name());
    let from_pandoc = kind == DocumentKind::Markdown
        && hit
            .file
            .file_name()
            .is_some_and(|name| name == generated_name.as_str());

    // pandoc's output is not a file the author has ever seen, so an answer in
    // it has to be carried back to the markdown through the anchors stored with
    // the build.
    let (relative, line) = if from_pandoc {
        let anchors = anchors::decode(&read_sidecar(&stored.pdf_path, "lines").unwrap_or_default());
        let Some(source) = anchors::source_line(&anchors, hit.line) else {
            // Above the first anchor: the preamble, or the title block pandoc
            // writes from the front matter. Nothing the author wrote inline.
            return Ok(None);
        };
        (project.file_name(), source)
    } else {
        let Some(relative) = locate(&hit.file, project, repository, &stored.summary.source_ref)?
        else {
            // A class or a package from the TeX distribution. Real, but not
            // this document, and not something to open.
            return Ok(None);
        };
        (relative, hit.line)
    };

    let Some(text) = read_source(project, repository, objects, &stored.summary.source_ref, &relative)?
    else {
        return Ok(None);
    };

    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(None);
    }
    let anchor = (line as usize).clamp(1, lines.len()) - 1;
    let (first, last) = if from_pandoc || kind == DocumentKind::Markdown {
        block_around(&lines, anchor)
    } else {
        construct_around(&lines, anchor)
    };

    Ok(Some(SourcePeek {
        file: relative,
        start_line: first as i64 + 1,
        end_line: last as i64 + 1,
        text: lines[first..=last].join("\n"),
    }))
}

struct Hit {
    file: PathBuf,
    line: u32,
}

/// Asks the `synctex` that ships with the same TeX distribution as latexmk.
/// Parsing the compressed format here would be a second implementation of
/// something already installed and already correct.
fn ask_synctex(pdf: &Path, page: u32, x: f64, y: f64) -> AppResult<Option<Hit>> {
    let synctex = resolve_executable("synctex").ok_or_else(|| {
        AppError::ToolUnavailable(
            "synctex was not found. It ships with TeX distributions, beside latexmk.".into(),
        )
    })?;
    let output = std::process::Command::new(&synctex)
        .env("PATH", augmented_path(&synctex))
        .arg("edit")
        .arg("-o")
        .arg(format!("{page}:{x:.2}:{y:.2}:{}", pdf.display()))
        .output()
        .map_err(|error| AppError::Build(format!("could not run synctex: {error}")))?;
    if !output.status.success() {
        return Ok(None);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut file = None;
    let mut line = None;
    for entry in text.lines() {
        if let Some(rest) = entry.strip_prefix("Input:") {
            file.get_or_insert_with(|| PathBuf::from(rest.trim()));
        } else if let Some(rest) = entry.strip_prefix("Line:") {
            line.get_or_insert(rest.trim().parse::<u32>().unwrap_or(0));
        }
    }
    match (file, line) {
        (Some(file), Some(line)) if line > 0 => Ok(Some(Hit {
            file: tidy(&file),
            line,
        })),
        _ => Ok(None),
    }
}

/// TeX writes the directory it was run in and the path it was given, so the
/// result usually has a `/./` in the middle of it.
fn tidy(path: &Path) -> PathBuf {
    path.components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect()
}

/// The path a hit names, relative to the project, or `None` when it is not part
/// of this document at all.
///
/// A snapshot was built in a temporary directory that no longer exists, so
/// there is no prefix left to strip. What it does have is a manifest of every
/// path it holds, and the hit ends with one of them.
fn locate(
    hit: &Path,
    project: &Project,
    repository: &Repository,
    source_ref: &SourceRef,
) -> AppResult<Option<String>> {
    match source_ref {
        SourceRef::Worktree => {
            // Both sides are resolved before they are compared. TeX records the
            // path it actually opened, and on macOS a project under `/tmp` or
            // `/var` is reached through a symlink, so the two spellings of the
            // same directory would otherwise never match.
            let directory = project.directory();
            let bases = [
                std::fs::canonicalize(&directory).unwrap_or_else(|_| directory.clone()),
                directory,
            ];
            let hits = [
                std::fs::canonicalize(hit).unwrap_or_else(|_| hit.to_path_buf()),
                hit.to_path_buf(),
            ];
            for base in &bases {
                for candidate in &hits {
                    if let Ok(relative) = candidate.strip_prefix(base) {
                        let relative = relative.to_string_lossy().replace('\\', "/");
                        if !relative.is_empty() {
                            return Ok(Some(relative));
                        }
                    }
                }
            }
            Ok(None)
        }
        SourceRef::Snapshot(revision) => {
            let manifest = repository.snapshot_manifest(project.id, revision)?;
            let hit = hit.to_string_lossy().replace('\\', "/");
            Ok(manifest
                .into_iter()
                .filter(|file| ends_at_boundary(&hit, &file.path))
                // The longest match wins: `two.tex` and `chapters/two.tex` can
                // both end the same path, and only one of them is the file.
                .max_by_key(|file| file.path.len())
                .map(|file| file.path))
        }
    }
}

fn ends_at_boundary(path: &str, candidate: &str) -> bool {
    if path == candidate {
        return true;
    }
    path.strip_suffix(candidate)
        .is_some_and(|prefix| prefix.ends_with('/'))
}

/// The source of one file, as this version has it.
fn read_source(
    project: &Project,
    repository: &Repository,
    objects: &Path,
    source_ref: &SourceRef,
    relative: &str,
) -> AppResult<Option<String>> {
    match source_ref {
        SourceRef::Worktree => Ok(std::fs::read_to_string(project.directory().join(relative)).ok()),
        SourceRef::Snapshot(revision) => {
            let manifest = repository.snapshot_manifest(project.id, revision)?;
            let Some(file) = manifest.into_iter().find(|file| file.path == relative) else {
                return Ok(None);
            };
            Ok(std::fs::read_to_string(snapshot::object_path(objects, &file.object)).ok())
        }
    }
}

fn read_sidecar(pdf: &Path, extension: &str) -> Option<String> {
    std::fs::read_to_string(pdf.with_extension(extension)).ok()
}

/// The whole environment a line belongs to, so that clicking an equation gives
/// back an equation rather than the line one of its boxes started on.
///
/// SyncTeX often answers with the closing line of a display, which is why this
/// looks in both directions before falling back to the paragraph.
fn construct_around(lines: &[&str], anchor: usize) -> (usize, usize) {
    if let Some(name) = environment_at(lines[anchor], "\\end{").filter(is_construct)
        && let Some(start) = scan_back(lines, anchor, &name)
    {
        return (start, anchor);
    }
    if let Some(name) = environment_at(lines[anchor], "\\begin{").filter(is_construct)
        && let Some(end) = scan_forward(lines, anchor, &name)
    {
        return (anchor, end);
    }
    // A paragraph of LaTeX ends where a structure does, not only at a blank
    // line: prose written straight under `\begin{document}` should come back
    // as prose, not with the preamble attached to it.
    paragraph(lines, anchor, |line| {
        line.contains("\\begin{") || line.contains("\\end{")
    })
}

/// The blank-line-delimited block a line sits in.
fn block_around(lines: &[&str], anchor: usize) -> (usize, usize) {
    paragraph(lines, anchor, |_| false)
}

/// Widens from a line to the run it belongs to. A boundary line stops the scan
/// and stays out of the result.
fn paragraph(lines: &[&str], anchor: usize, boundary: impl Fn(&str) -> bool) -> (usize, usize) {
    let ends = |line: &str| line.trim().is_empty() || boundary(line);
    let mut first = anchor;
    while first > 0 && !ends(lines[first - 1]) && anchor - first < MOST_LINES {
        first -= 1;
    }
    let mut last = anchor;
    while last + 1 < lines.len() && !ends(lines[last + 1]) && last - anchor < MOST_LINES {
        last += 1;
    }
    (first, last)
}

/// `document` wraps everything, so widening to it would answer every click with
/// the whole file. TeX attributes the first box on a page to it often enough
/// for that to matter.
fn is_construct(name: &String) -> bool {
    name != "document"
}

fn environment_at(line: &str, opener: &str) -> Option<String> {
    let rest = line.split_once(opener)?.1;
    let name = rest.split_once('}')?.0;
    (!name.is_empty()).then(|| name.to_owned())
}

fn scan_back(lines: &[&str], from: usize, name: &str) -> Option<usize> {
    let open = format!("\\begin{{{name}}}");
    let close = format!("\\end{{{name}}}");
    let mut depth = 0_i32;
    for index in (0..from).rev() {
        if from - index > MOST_LINES {
            return None;
        }
        if lines[index].contains(&close) {
            depth += 1;
        } else if lines[index].contains(&open) {
            if depth == 0 {
                return Some(index);
            }
            depth -= 1;
        }
    }
    None
}

fn scan_forward(lines: &[&str], from: usize, name: &str) -> Option<usize> {
    let open = format!("\\begin{{{name}}}");
    let close = format!("\\end{{{name}}}");
    let mut depth = 0_i32;
    for (index, line) in lines.iter().enumerate().skip(from + 1) {
        if index - from > MOST_LINES {
            return None;
        }
        if line.contains(&open) {
            depth += 1;
        } else if line.contains(&close) {
            if depth == 0 {
                return Some(index);
            }
            depth -= 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &[&str] = &[
        "\\documentclass{article}",       // 0
        "\\begin{document}",              // 1
        "Some prose that runs",           // 2
        "over two lines.",                // 3
        "",                               // 4
        "\\begin{equation}",              // 5
        "  E = mc^2",                     // 6
        "\\end{equation}",                // 7
        "",                               // 8
        "More prose.",                    // 9
        "\\end{document}",                // 10
    ];

    /// SyncTeX answers a click on a display with the line its box closed on,
    /// so the widening has to work backwards as well as forwards.
    #[test]
    fn an_equation_comes_back_whole_from_either_end() {
        assert_eq!(construct_around(DOCUMENT, 7), (5, 7), "from \\end");
        assert_eq!(construct_around(DOCUMENT, 5), (5, 7), "from \\begin");
    }

    /// Prose comes back as its paragraph and nothing else — `\begin{document}`
    /// above it and `\end{document}` below it are structure, not content.
    #[test]
    fn prose_comes_back_as_its_paragraph() {
        assert_eq!(construct_around(DOCUMENT, 2), (2, 3));
        assert_eq!(construct_around(DOCUMENT, 3), (2, 3));
        assert_eq!(construct_around(DOCUMENT, 9), (9, 9));
    }

    /// Markdown has no environments to stop at, so a block runs to the blank
    /// lines around it.
    #[test]
    fn a_markdown_block_runs_to_the_blank_lines_around_it() {
        let lines = ["# Heading", "", "A paragraph", "over two lines.", "", "After."];
        assert_eq!(block_around(&lines, 2), (2, 3));
        assert_eq!(block_around(&lines, 0), (0, 0));
        assert_eq!(block_around(&lines, 5), (5, 5));
    }

    #[test]
    fn nested_environments_match_their_own_end() {
        let lines = [
            "\\begin{figure}",
            "\\begin{tabular}{cc}",
            "a & b",
            "\\end{tabular}",
            "\\caption{Two}",
            "\\end{figure}",
        ];
        assert_eq!(construct_around(&lines, 5), (0, 5), "the outer one");
        assert_eq!(construct_around(&lines, 3), (1, 3), "the inner one");
    }

    /// A snapshot's hit carries the temporary directory it was built in, which
    /// is gone. What is left is a path ending in one the manifest knows.
    #[test]
    fn a_manifest_path_is_matched_at_a_separator() {
        assert!(ends_at_boundary("/tmp/press-1/chapters/two.tex", "chapters/two.tex"));
        assert!(ends_at_boundary("main.tex", "main.tex"));
        assert!(
            !ends_at_boundary("/tmp/press-1/notes/two.tex", "s/two.tex"),
            "a match has to start at a path separator"
        );
    }

    #[test]
    fn a_hit_loses_the_dot_tex_puts_in_the_middle_of_it() {
        assert_eq!(
            tidy(Path::new("/projects/thesis/./chapters/two.tex")),
            PathBuf::from("/projects/thesis/chapters/two.tex")
        );
    }

    // -- the whole path, against the real toolchain ------------------------

    use crate::{
        model::{ArtifactSummary, Engine},
        runner::{self, BuildInputs, BuildOutcome, CancelHandle, PidRegistry, ProgressSink},
        toolchain::resolve_executable,
    };
    use std::sync::Arc;

    fn project_at(document: &Path) -> Project {
        Project {
            id: 1,
            name: "Fixture".into(),
            document_path: document.to_string_lossy().into_owned(),
            engine: Engine::PdfLatex,
            pinned: false,
            created_at: 0,
            last_opened_at: 0,
        }
    }

    /// Builds a document for real and hands back what a click would be given.
    async fn built(root: &Path, work: &Path, artifacts: &Path, project: &Project) -> StoredArtifact {
        let repository = Repository::open(&root.join("../press.db")).unwrap();
        let source = crate::sources::prepare(project, &SourceRef::Worktree, &repository, root)
            .unwrap();
        let (_handle, cancel) = CancelHandle::new();
        let sink: ProgressSink = Arc::new(|_| {});
        let outcome = runner::run(
            BuildInputs {
                build_id: 1,
                project,
                source: &source,
                work_directory: work.to_path_buf(),
                log_path: work.join("last-build.log"),
                artifact_directory: artifacts.to_path_buf(),
            },
            cancel,
            Arc::new(PidRegistry::default()),
            sink,
        )
        .await
        .unwrap();
        let BuildOutcome::Succeeded { product, .. } = outcome else {
            panic!("the fixture should compile");
        };
        StoredArtifact {
            summary: ArtifactSummary {
                id: 1,
                project_id: project.id,
                source_ref: SourceRef::Worktree,
                engine: project.engine,
                page_count: product.page_count,
                byte_size: product.byte_size,
                built_at: 0,
                revision: 1,
            },
            pdf_path: product.pdf_path,
        }
    }

    /// Sweeps a grid over the page and collects every answer. Where a line
    /// lands is the typesetter's business, not this test's, and a centred
    /// display sits nowhere near a paragraph's left edge.
    fn sweep(
        project: &Project,
        stored: &StoredArtifact,
        repository: &Repository,
        objects: &Path,
    ) -> Vec<SourcePeek> {
        let mut found = Vec::new();
        for x in [150.0, 306.0] {
            for step in 8..60 {
                if let Some(peek) =
                    resolve(project, stored, repository, objects, 1, x, f64::from(step) * 10.0)
                        .unwrap()
                {
                    found.push(peek);
                }
            }
        }
        found
    }

    /// The whole feature for LaTeX: build, publish the sync data with the PDF,
    /// and resolve a point on the page back to the source that made it.
    #[tokio::test]
    async fn a_click_on_an_equation_gives_back_the_equation() {
        if resolve_executable("latexmk").is_none() || resolve_executable("synctex").is_none() {
            eprintln!("skipping: latexmk or synctex is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("source");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("main.tex"),
            "\\documentclass{article}\n\
             \\begin{document}\n\n\
             Prose above the equation.\n\n\
             \\begin{equation}\n  E = mc^2\n\\end{equation}\n\n\
             Prose below it.\n\
             \\end{document}\n",
        )
        .unwrap();

        let project = project_at(&root.join("main.tex"));
        let work = directory.path().join("work");
        let artifacts = directory.path().join("artifacts");
        let stored = built(&root, &work, &artifacts, &project).await;

        assert!(
            stored.pdf_path.with_extension("synctex.gz").is_file(),
            "the sync data travels with the PDF, not with the scratch directory"
        );

        let repository = Repository::open(&directory.path().join("db")).unwrap();
        let found = sweep(&project, &stored, &repository, directory.path());
        assert!(!found.is_empty(), "some point on the page has a source");
        assert!(
            found.iter().all(|peek| peek.file == "main.tex"),
            "every answer names the document: {found:?}"
        );
        let equation = found
            .iter()
            .find(|peek| peek.text.contains("E = mc^2"))
            .expect("the equation is somewhere on the page");
        assert!(
            equation.text.starts_with("\\begin{equation}")
                && equation.text.trim_end().ends_with("\\end{equation}"),
            "an equation comes back whole: {:?}",
            equation.text
        );
        assert!(
            found.iter().any(|peek| peek.text == "Prose above the equation."),
            "prose comes back as its paragraph: {found:?}"
        );
    }

    /// The same for markdown, where SyncTeX can only name pandoc's output: the
    /// answer has to arrive as markdown, in the file the author wrote.
    #[tokio::test]
    async fn a_click_in_a_markdown_document_gives_back_markdown() {
        if resolve_executable("latexmk").is_none()
            || resolve_executable("pandoc").is_none()
            || resolve_executable("synctex").is_none()
        {
            eprintln!("skipping: latexmk, pandoc or synctex is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("source");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("essay.md"),
            "---\ntitle: An Essay\n---\n\n\
             # Introduction\n\n\
             A paragraph of markdown prose.\n\n\
             $$E = mc^2 + \\alpha$$\n\n\
             A closing paragraph.\n",
        )
        .unwrap();

        let project = project_at(&root.join("essay.md"));
        let work = directory.path().join("work");
        let artifacts = directory.path().join("artifacts");
        let stored = built(&root, &work, &artifacts, &project).await;

        assert!(
            stored.pdf_path.with_extension("lines").is_file(),
            "the anchors travel with the PDF too"
        );

        let repository = Repository::open(&directory.path().join("db")).unwrap();
        let found = sweep(&project, &stored, &repository, directory.path());
        assert!(!found.is_empty(), "some point on the page has a source");
        assert!(
            found.iter().all(|peek| peek.file == "essay.md"),
            "the answer is the markdown, never pandoc's LaTeX: {found:?}"
        );
        assert!(
            found.iter().all(|peek| !peek.text.contains("\\section")),
            "no line of pandoc's output reaches the reader: {found:?}"
        );
        assert!(
            found
                .iter()
                .any(|peek| peek.text == "A paragraph of markdown prose."),
            "prose comes back as the block the author wrote: {found:?}"
        );
        assert!(
            found.iter().any(|peek| peek.text.contains("$$E = mc^2")),
            "and so does the display equation: {found:?}"
        );
    }
}
