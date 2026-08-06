//! Turning a path from outside into a project to open.
//!
//! The editor knows which file is being edited; it does not know which project
//! that file belongs to, and it should not have to. `:Press` from inside
//! `chapters/three.tex` sends that path, and working out that the project is the
//! directory above — the one holding `main.tex` — is Press's job, using the same
//! discovery that the folder picker uses.

use std::path::{Path, PathBuf};

use crate::{
    database::Repository,
    discovery,
    error::{AppError, AppResult},
    model::{DocumentKind, OpenRequest},
};

/// How far above the file to look for a document root before giving up. A paper
/// nested deeper than this inside its own project is not a layout worth guessing
/// at.
const MAX_ASCENT: usize = 4;

/// Resolves a path to either a project Press already knows, or a discovery
/// report the interface can turn into a new one.
pub fn resolve(path: &Path, repository: &Repository) -> AppResult<OpenRequest> {
    let canonical = path.canonicalize().map_err(|error| {
        AppError::NotFound(format!("{} cannot be opened: {error}", path.display()))
    })?;
    let display = canonical.to_string_lossy().into_owned();

    // A file already inside a known project opens that project, whatever else
    // discovery might make of the folder.
    if let Some(project_id) = enclosing_project(&canonical, repository)? {
        return Ok(OpenRequest {
            path: display,
            project_id: Some(project_id),
            report: None,
            message: None,
        });
    }

    // A markdown file named directly is the document. Press never looks at a
    // folder and guesses which markdown in it is the document — markdown carries
    // none of the evidence LaTeX does, so any ranking would be invention. Naming
    // the file is the only reliable signal there is.
    if !canonical.is_dir() && DocumentKind::of(&canonical) == DocumentKind::Markdown {
        return Ok(OpenRequest {
            path: display,
            project_id: None,
            report: Some(discovery::document_report(&canonical)?),
            message: None,
        });
    }

    let start = if canonical.is_dir() {
        canonical.clone()
    } else {
        canonical.parent().unwrap_or(&canonical).to_path_buf()
    };

    // The file the editor named, so a folder holding several documents does not
    // stop to ask which one was meant.
    let named = (!canonical.is_dir()).then(|| canonical.clone());

    let mut directory = start.clone();
    for _ in 0..=MAX_ASCENT {
        if let Ok(mut report) = discovery::inspect(&directory)
            && !report.candidates.is_empty()
        {
            prefer_named_file(&mut report, &directory, named.as_deref());
            return Ok(OpenRequest {
                path: display,
                project_id: None,
                report: Some(report),
                message: None,
            });
        }
        // A repository boundary is as far out as a single paper ever reaches.
        if directory.join(".git").exists() {
            break;
        }
        match directory.parent() {
            Some(parent) if parent != directory => directory = parent.to_path_buf(),
            _ => break,
        }
    }

    Ok(OpenRequest {
        path: display,
        project_id: None,
        report: None,
        // Says what to do next, because the alternative — guessing at markdown —
        // is exactly what makes this unreliable.
        message: Some(format!(
            "No LaTeX document was found in {} or the folders above it. \
             To open a markdown document, run :Press from that file itself.",
            start.display()
        )),
    })
}

/// When the editor named a file that is itself a candidate, that is the answer:
/// the user has already said which document they meant, so there is nothing left
/// to disambiguate.
fn prefer_named_file(
    report: &mut crate::model::DiscoveryReport,
    root: &Path,
    named: Option<&Path>,
) {
    let Some(named) = named else { return };
    let Ok(relative) = named.strip_prefix(root) else {
        return;
    };
    let relative = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/");
    if report
        .candidates
        .iter()
        .any(|candidate| candidate.relative_path == relative)
    {
        report.recommended_main = Some(relative);
        report.requires_selection = false;
    }
}

/// The known project this path belongs to.
///
/// A folder can hold several documents, so containment alone is not an answer.
/// Naming a project's own main file picks that project outright; otherwise the
/// innermost folder wins, and between documents sharing that folder, whichever
/// was read most recently.
fn enclosing_project(path: &Path, repository: &Repository) -> AppResult<Option<i64>> {
    let mut best: Option<(usize, i64, i64)> = None;
    for location in repository.project_locations()? {
        let root = PathBuf::from(&location.root_path);
        if !path.starts_with(&root) {
            continue;
        }
        // The path is this project's document: nothing beats that.
        if root.join(&location.main_file) == path {
            return Ok(Some(location.id));
        }
        let depth = root.components().count();
        let better = match best {
            None => true,
            Some((current_depth, _, current_opened)) => {
                depth > current_depth
                    || (depth == current_depth && location.last_opened_at > current_opened)
            }
        };
        if better {
            best = Some((depth, location.id, location.last_opened_at));
        }
    }
    Ok(best.map(|(_, id, _)| id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::NewProject, model::{DocumentKind, Engine}};

    fn repository(directory: &Path) -> Repository {
        Repository::open(&directory.join("press.db")).unwrap()
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn a_file_in_a_known_project_opens_that_project() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("thesis");
        write(&root, "main.tex", "\\documentclass{book}\n");
        write(&root, "chapters/one.tex", "text\n");
        let repository = repository(directory.path());
        let project = repository
            .upsert_project(NewProject {
                name: "Thesis",
                root_path: root.canonicalize().unwrap().to_str().unwrap(),
                main_file: "main.tex",
                working_directory: ".",
                kind: DocumentKind::Latex,
                engine: Engine::PdfLatex,
            })
            .unwrap();

        let request = resolve(&root.join("chapters/one.tex"), &repository).unwrap();
        assert_eq!(request.project_id, Some(project.id));
        assert!(request.report.is_none());
    }

    #[test]
    fn an_unknown_file_yields_a_discovery_report_for_its_project() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "main.tex", "\\documentclass{article}\n\\begin{document}\\end{document}\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("main.tex"), &repository).unwrap();
        assert_eq!(request.project_id, None);
        let report = request.report.expect("a report for a new project");
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert!(report.root_path.ends_with("paper"));
    }

    #[test]
    fn a_chapter_resolves_to_the_folder_holding_the_document() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "main.tex", "\\documentclass{book}\n\\include{chapters/three}\n");
        write(&root, "chapters/three.tex", "the third chapter\n");
        let repository = repository(directory.path());

        // Editing a chapter, which is not itself a document root.
        let request = resolve(&root.join("chapters/three.tex"), &repository).unwrap();
        let report = request.report.expect("walked up to the document");
        assert!(report.root_path.ends_with("paper"), "{}", report.root_path);
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
    }

    #[test]
    fn a_paper_inside_a_repository_resolves_to_the_paper_not_the_repository() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("research");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write(&repo, "README.md", "a big repository\n");
        write(&repo, "papers/thesis/main.tex", "\\documentclass{book}\n");
        let repository = repository(directory.path());

        let request = resolve(&repo.join("papers/thesis/main.tex"), &repository).unwrap();
        let report = request.report.unwrap();
        assert!(report.root_path.ends_with("papers/thesis"), "{}", report.root_path);
    }

    #[test]
    fn a_directory_can_be_opened_directly() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "main.tex", "\\documentclass{article}\n");
        let repository = repository(directory.path());

        let request = resolve(&root, &repository).unwrap();
        assert!(request.report.is_some());
    }

    #[test]
    fn a_markdown_file_named_directly_is_the_document() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("writing");
        write(&root, "essay.md", "# An essay\n");
        // Neighbours that a folder scan would have had to rank somehow.
        write(&root, "talk.md", "# A talk\n");
        write(&root, "README.md", "how to build this\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("essay.md"), &repository).unwrap();
        let report = request.report.expect("a named markdown file is a document");
        assert_eq!(report.recommended_main.as_deref(), Some("essay.md"));
        assert_eq!(report.candidates.len(), 1, "no guessing at the neighbours");
        assert_eq!(report.candidates[0].kind, DocumentKind::Markdown);
        assert!(!report.requires_selection);
        assert!(report.root_path.ends_with("writing"));
    }

    /// The rule that keeps this predictable: markdown is never inferred from a
    /// folder, only ever from being named.
    #[test]
    fn a_folder_of_markdown_is_not_guessed_at() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("writing");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write(&root, "essay.md", "# An essay\n");
        write(&root, "talk.md", "# A talk\n");
        let repository = repository(directory.path());

        // Opening the folder, rather than a file in it, finds no LaTeX and says
        // so instead of picking a markdown file at random.
        let request = resolve(&root, &repository).unwrap();
        assert!(request.report.is_none());
        let message = request.message.unwrap();
        assert!(message.contains("No LaTeX document"), "{message}");
        assert!(
            message.contains(":Press from that file itself"),
            "the failure has to say how to open markdown: {message}"
        );
    }

    #[test]
    fn a_non_document_file_does_not_become_a_markdown_project() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write(&root, "results.csv", "a,b\n1,2\n");
        write(&root, "README.md", "documents the folder\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("results.csv"), &repository).unwrap();
        assert!(request.report.is_none());
        assert!(request.message.unwrap().contains("No LaTeX document"));
    }

    #[test]
    fn markdown_beside_a_latex_paper_still_opens_the_paper_when_the_tex_is_named() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        write(&root, "main.tex", "\\documentclass{article}\n");
        write(&root, "notes.md", "working notes\n");
        let repository = repository(directory.path());

        let request = resolve(&root.join("main.tex"), &repository).unwrap();
        let report = request.report.unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert_eq!(report.candidates[0].kind, DocumentKind::Latex);
    }

    #[test]
    fn a_path_that_does_not_exist_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        let repository = repository(directory.path());
        assert!(resolve(&directory.path().join("absent.tex"), &repository).is_err());
    }
}
