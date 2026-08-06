//! Resolving a [`SourceRef`] to a directory that can be compiled.
//!
//! The working tree is compiled where it lives. A snapshot has to be
//! materialized somewhere first, and that is the only difference between
//! building the live document and building a version from the history — which is
//! why everything downstream takes a [`PreparedSource`] rather than a project.

use std::path::{Path, PathBuf};

use crate::{
    database::Repository,
    error::{AppError, AppResult},
    model::{Project, SourceRef},
    snapshot,
};

/// Where a build runs and what it compiles.
///
/// Two fields, because a project is a document: latexmk runs in the document's
/// own directory and is handed the document's own name. A snapshot swaps the
/// directory for a checkout and changes nothing else.
#[derive(Debug)]
pub struct PreparedSource {
    /// Where latexmk runs, and what log paths are reported relative to.
    pub directory: PathBuf,
    /// The document, relative to `directory`.
    pub file_name: String,
    /// Held for the lifetime of the build; dropping it removes a checkout.
    _checkout: Option<tempfile::TempDir>,
}

impl PreparedSource {
    pub fn document(&self) -> PathBuf {
        self.directory.join(&self.file_name)
    }
}

/// Prepares a version of a project for compilation.
///
/// The working tree is compiled where it lives. A snapshot is restored into a
/// temporary directory that lives exactly as long as the build, so a version
/// from the history is compiled from its own copy and the project folder is
/// never involved.
pub fn prepare(
    project: &Project,
    source_ref: &SourceRef,
    repository: &Repository,
    objects: &Path,
) -> AppResult<PreparedSource> {
    let file_name = project.file_name();
    match source_ref {
        SourceRef::Worktree => {
            if !project.document().is_file() {
                return Err(AppError::NotFound(format!(
                    "{} is no longer there",
                    project.document_path
                )));
            }
            Ok(PreparedSource {
                directory: project.directory(),
                file_name,
                _checkout: None,
            })
        }
        SourceRef::Snapshot(revision) => {
            let manifest = repository.snapshot_manifest(project.id, revision)?;
            let checkout = tempfile::Builder::new().prefix("press-version-").tempdir()?;
            snapshot::materialize(&manifest, objects, checkout.path())?;

            if !checkout.path().join(&file_name).is_file() {
                // The project's document was renamed after this version was
                // stored, so there is nothing here to compile.
                return Err(AppError::NotFound(format!(
                    "this version does not contain {file_name}"
                )));
            }
            Ok(PreparedSource {
                directory: checkout.path().to_path_buf(),
                file_name,
                _checkout: Some(checkout),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Engine;
    use std::{collections::HashSet, path::Path};

    fn empty_store() -> (Repository, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::open(&directory.path().join("press.db")).unwrap();
        (repository, directory)
    }

    fn project(document: &Path) -> Project {
        Project {
            id: 1,
            name: "Fixture".into(),
            document_path: document.to_string_lossy().into_owned(),
            engine: Engine::PdfLatex,
            created_at: 0,
            last_opened_at: 0,
        }
    }

    #[test]
    fn the_working_tree_is_compiled_in_its_own_directory() {
        let directory = tempfile::tempdir().unwrap();
        let papers = directory.path().join("papers");
        std::fs::create_dir_all(&papers).unwrap();
        std::fs::write(papers.join("main.tex"), "\\documentclass{article}").unwrap();

        let project = project(&papers.join("main.tex"));
        let (repository, _store) = empty_store();
        let prepared =
            prepare(&project, &SourceRef::Worktree, &repository, Path::new("/objects")).unwrap();
        // The document's own folder, whatever sits above it.
        assert_eq!(prepared.directory, papers);
        assert_eq!(prepared.file_name, "main.tex");
    }

    #[test]
    fn a_vanished_document_is_reported_before_latexmk_runs() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(&directory.path().join("main.tex"));
        let (repository, _store) = empty_store();
        let error =
            prepare(&project, &SourceRef::Worktree, &repository, Path::new("/objects")).unwrap_err();
        assert!(error.to_string().contains("main.tex"));
    }

    /// The whole point of the feature: a version built from the history is the
    /// source as it was, not as it is now.
    #[test]
    fn a_snapshot_is_restored_as_it_was_not_as_the_folder_is_now() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        let objects = directory.path().join("objects");
        std::fs::create_dir_all(root.join("chapters")).unwrap();
        std::fs::write(root.join("main.tex"), "\\documentclass{article}\noriginal\n").unwrap();
        std::fs::write(root.join("chapters/one.tex"), "first draft\n").unwrap();

        let repository = Repository::open(&directory.path().join("press.db")).unwrap();
        let stored = repository
            .upsert_project(crate::database::NewProject {
                name: "paper/main.tex",
                document_path: root.join("main.tex").to_str().unwrap(),
                engine: Engine::PdfLatex,
            })
            .unwrap();

        let capture = crate::snapshot::capture(&root, &objects, &HashSet::new()).unwrap();
        let snapshot = repository
            .create_snapshot(stored.id, &capture, "  First draft  ", Some("  "))
            .unwrap();
        assert_eq!(snapshot.title, "First draft", "titles are trimmed");
        assert_eq!(snapshot.body, None, "a blank body is not stored");
        assert_eq!(snapshot.file_count, 2);

        // The working tree moves on.
        std::fs::write(root.join("main.tex"), "\\documentclass{article}\nrewritten\n").unwrap();
        std::fs::remove_file(root.join("chapters/one.tex")).unwrap();

        let prepared = prepare(
            &stored,
            &SourceRef::Snapshot(snapshot.revision.clone()),
            &repository,
            &objects,
        )
        .unwrap();

        assert_ne!(
            prepared.directory, root,
            "a version never builds in the project folder"
        );
        assert_eq!(
            std::fs::read_to_string(prepared.document()).unwrap(),
            "\\documentclass{article}\noriginal\n"
        );
        assert_eq!(
            std::fs::read_to_string(prepared.directory.join("chapters/one.tex")).unwrap(),
            "first draft\n",
            "a file deleted since the snapshot still comes back"
        );
        assert_eq!(prepared.file_name, "main.tex");

        // The history is listed with the working tree pinned at the top.
        let versions = repository.list_versions(stored.id).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].source_ref, SourceRef::Worktree);
        assert_eq!(versions[0].title, "Working tree");
        assert_eq!(versions[1].title, "First draft");
        assert!(versions[1].artifact.is_none(), "not built yet");

        // The checkout goes away with the build that owns it.
        let checkout = prepared.directory.clone();
        drop(prepared);
        assert!(!checkout.exists());
    }

    #[test]
    fn a_version_missing_the_document_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        let objects = directory.path().join("objects");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("notes.tex"), "just notes\n").unwrap();

        let repository = Repository::open(&directory.path().join("press.db")).unwrap();
        let stored = repository
            .upsert_project(crate::database::NewProject {
                name: "paper/main.tex",
                // The snapshot below will not contain this file.
                document_path: root.join("main.tex").to_str().unwrap(),
                engine: Engine::PdfLatex,
            })
            .unwrap();
        let capture = crate::snapshot::capture(&root, &objects, &HashSet::new()).unwrap();
        let snapshot = repository
            .create_snapshot(stored.id, &capture, "Notes only", None)
            .unwrap();

        let error = prepare(
            &stored,
            &SourceRef::Snapshot(snapshot.revision),
            &repository,
            &objects,
        )
        .unwrap_err();
        assert!(error.to_string().contains("main.tex"), "{error}");
    }

    #[test]
    fn an_unknown_revision_is_reported_rather_than_built() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.tex"), "").unwrap();
        let project = project(&directory.path().join("main.tex"));
        let (repository, _store) = empty_store();
        let error = prepare(
            &project,
            &SourceRef::Snapshot("abc123".into()),
            &repository,
            Path::new("/objects"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("abc123"));
    }
}
