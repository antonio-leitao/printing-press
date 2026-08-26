//! One document kept so that the next can begin as a copy of it.
//!
//! A template is a snapshot that belongs to Press rather than to a project. It
//! is stored by [`snapshot::capture`], placed by [`snapshot::materialize`], and
//! shares the object store with the history — so a template made from a thesis
//! that has already been snapshotted costs almost nothing, and five templates
//! carrying the same class file carry it once.
//!
//! That reuse is the whole design. "Which files belong to this document" is a
//! question Press already answers, in [`files::belongs_to_project`], and it is
//! the same question a template asks: keep the source, the figures, the `.bib`
//! and the included chapters; keep out build output, editor scratch, `.git`,
//! and the documents of other projects sharing the folder. Answering it twice
//! would mean two answers to drift apart.
//!
//! What a template does *not* do is edit what it holds. It is what the project
//! was at the moment it was saved — preamble, body and all. Blanking the body
//! of the entry document would mean parsing it and guessing, and a template
//! made from a finished paper is a legitimate thing to want. Starting blank is
//! a separate creation choice and does not put a synthetic template in here.

use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{
    error::{AppError, AppResult},
    model::{DocumentKind, ProjectFile},
    snapshot::{self, StoredFile},
};

/// What Press writes while it is assembling a new document, before anything
/// under the reader's chosen name exists. Inside the destination folder rather
/// than in a temporary directory elsewhere, so the move into place is a rename
/// on one filesystem rather than a copy across two.
const STAGING: &str = ".press-new";

/// Reads the chosen files of a project into the object store.
///
/// `chosen` comes from the interface, where the reader has just struck things
/// out of the tree, so it is intersected with Press's own walk rather than
/// trusted: a path that walk does not produce is not part of this project, and
/// naming one here should not be a way to reach it. That covers the build
/// output and the neighbour's draft as well as the traversal — all three are
/// simply absent from the answer.
pub fn capture(
    directory: &Path,
    entry: &str,
    objects: &Path,
    foreign: &HashSet<String>,
    chosen: &[String],
) -> AppResult<Vec<StoredFile>> {
    let chosen = chosen.iter().collect::<HashSet<_>>();
    let keeping = snapshot::list(directory, foreign)?
        .into_iter()
        .filter(|file| chosen.contains(&file.path))
        .collect::<Vec<_>>();

    // A template without its document is not a template. The tree keeps this
    // one ticked and refuses to untick it, so reaching here means something
    // other than the interface asked.
    if !keeping.iter().any(|file| file.path == entry) {
        return Err(AppError::InvalidInput(format!(
            "a template has to keep {entry}, the document itself"
        )));
    }
    Ok(snapshot::store_all(directory, objects, &keeping)?.files)
}

/// What the tree offers: every file this document would compile with.
pub fn listing(directory: &Path, foreign: &HashSet<String>) -> AppResult<Vec<ProjectFile>> {
    snapshot::list(directory, foreign)
}

/// Starts one document without stationery.
///
/// A blank document follows the same placement rule as a one-file template:
/// the name is the file name and the chosen folder is its parent. Markdown is
/// genuinely empty. LaTeX needs a document environment, and `\null` gives the
/// viewer one blank page until there is something of the reader's to show.
pub fn create_blank(kind: DocumentKind, folder: &Path, name: &str) -> AppResult<PathBuf> {
    if !folder.is_dir() {
        return Err(AppError::NotFound(format!(
            "{} is not a folder",
            folder.display()
        )));
    }
    let name = validate_name(name)?;
    let (extension, contents): (&str, &[u8]) = match kind {
        DocumentKind::Latex => (
            "tex",
            b"\\documentclass{article}\n\n\\begin{document}\n\n\\null\n\n\\end{document}\n",
        ),
        DocumentKind::Markdown => ("md", b""),
    };
    let document = folder.join(format!("{name}.{extension}"));

    // `create_new` is the no-overwrite promise made atomically. Checking first
    // and then writing would leave a race in which a file arriving between the
    // two could be replaced.
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&document)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(AppError::InvalidInput(format!(
                "{} already exists",
                document
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| document.display().to_string())
            )));
        }
        Err(error) => return Err(error.into()),
    };
    if let Err(error) = file.write_all(contents) {
        drop(file);
        let _ = std::fs::remove_file(&document);
        return Err(error.into());
    }
    Ok(document)
}

/// Where a template's files will land, and under what name.
///
/// The shape follows the file count rather than the kind of document. One file
/// is written into the folder the reader chose, because Press keeps auxiliary
/// files in its own cache and a lone document needs no folder to stay tidy.
/// Several files get a folder of their own, which is both what anyone would
/// expect of a project and what makes the collision check a single question:
/// whether that one name is taken.
pub struct Placement {
    /// The folder Press will create, when the template brings more than one file.
    pub folder: Option<PathBuf>,
    /// The document itself, once everything is in place.
    pub document: PathBuf,
}

/// Works out where a template would go without putting anything there.
///
/// Separate from [`create`] because the interface wants to say what will happen
/// — and what is in the way — while the name is still being typed.
pub fn placement(files: &[StoredFile], entry: &str, folder: &Path, name: &str) -> AppResult<Placement> {
    let name = validate_name(name)?;
    let extension = Path::new(entry)
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| AppError::InvalidInput("this template has no document in it".into()))?;

    if files.len() == 1 {
        Ok(Placement {
            folder: None,
            document: folder.join(format!("{name}.{extension}")),
        })
    } else {
        let root = folder.join(name);
        Ok(Placement {
            document: root.join(format!("{name}.{extension}")),
            folder: Some(root),
        })
    }
}

/// Places a template's files and answers with the document among them.
///
/// Nothing is ever written over. A name already taken is refused by name, which
/// is a question the reader can answer, rather than merged into — Press has one
/// promise about the folders it is pointed at, and quietly replacing a file in
/// one would be a poor time to break it.
///
/// Assembled under a staging name and moved into place with a single rename, so
/// a failure halfway through leaves the folder as it was rather than half a
/// project the reader now has to identify and clear out.
pub fn create(
    files: &[StoredFile],
    entry: &str,
    objects: &Path,
    folder: &Path,
    name: &str,
) -> AppResult<PathBuf> {
    if !folder.is_dir() {
        return Err(AppError::NotFound(format!(
            "{} is not a folder",
            folder.display()
        )));
    }
    let placed = placement(files, entry, folder, name)?;
    let destination = placed.folder.as_deref().unwrap_or(&placed.document);
    // `symlink_metadata`, so a broken symlink counts as something being there.
    // It is still a name that is taken, and following it would be worse.
    if destination.symlink_metadata().is_ok() {
        return Err(AppError::InvalidInput(format!(
            "{} already exists",
            destination
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| destination.display().to_string())
        )));
    }

    let staging = folder.join(STAGING);
    // A staging directory left by a run that was killed between assembling and
    // renaming. Nothing of the reader's is in it — the name is Press's own.
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)?;

    let assembled = (|| -> AppResult<()> {
        snapshot::materialize(files, objects, &staging)?;
        // The entry document takes the name the reader typed; everything beside
        // it keeps the name it had, because that is what the document refers to
        // it by.
        let named = staging.join(
            placed
                .document
                .file_name()
                .ok_or_else(|| AppError::InvalidInput("that name cannot be a file".into()))?,
        );
        let written = staging.join(entry);
        if written != named {
            std::fs::rename(&written, &named)?;
        }
        match &placed.folder {
            // The folder is the staging directory, renamed.
            Some(root) => std::fs::rename(&staging, root)?,
            None => std::fs::rename(&named, &placed.document)?,
        }
        Ok(())
    })();

    // Whether it worked or not: on success the single-file case leaves an empty
    // staging directory behind, and on failure there is a part-built one.
    let _ = std::fs::remove_dir_all(&staging);
    assembled?;
    Ok(placed.document)
}

/// A name Press can write into a folder.
///
/// Refused rather than corrected. A name with a slash in it is someone asking
/// for a folder they have already chosen elsewhere in the dialog, and silently
/// flattening it would put the document somewhere they did not ask for.
pub fn validate_name(name: &str) -> AppResult<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("a document needs a name".into()));
    }
    if name.starts_with('.') {
        return Err(AppError::InvalidInput(
            "a name beginning with a dot is hidden; Press will not create one".into(),
        ));
    }
    if name.contains(['/', '\\', ':']) {
        return Err(AppError::InvalidInput(
            "a name cannot contain a slash or a colon — the folder is chosen above".into(),
        ));
    }
    if name == STAGING {
        return Err(AppError::InvalidInput(
            "that name is one Press uses while it works".into(),
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project and its object store, with `main.tex` beside a class file and
    /// a figure — the shape the two scopes disagree about.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        std::fs::create_dir_all(root.join("figures")).unwrap();
        std::fs::write(root.join("main.tex"), "\\documentclass{article}").unwrap();
        std::fs::write(root.join("thesis.cls"), "% a class").unwrap();
        std::fs::write(root.join("figures/plot.pdf"), "%PDF-1.4").unwrap();
        // Build output, which belongs to neither scope.
        std::fs::write(root.join("main.aux"), "\\relax").unwrap();
        let objects = directory.path().join("objects");
        std::fs::create_dir_all(&objects).unwrap();
        (directory, root, objects)
    }

    /// Capturing a selection, which is all any of these want to say.
    fn keep(root: &Path, objects: &Path, chosen: &[&str]) -> Vec<StoredFile> {
        let chosen = chosen.iter().map(|path| (*path).to_owned()).collect::<Vec<_>>();
        capture(root, "main.tex", objects, &HashSet::new(), &chosen).unwrap()
    }

    #[test]
    fn blank_documents_use_their_kind_and_never_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let markdown = create_blank(DocumentKind::Markdown, directory.path(), "notes").unwrap();
        assert_eq!(markdown, directory.path().join("notes.md"));
        assert_eq!(std::fs::read(&markdown).unwrap(), b"");

        let latex = create_blank(DocumentKind::Latex, directory.path(), "paper").unwrap();
        assert_eq!(latex, directory.path().join("paper.tex"));
        let source = std::fs::read_to_string(&latex).unwrap();
        assert!(source.contains("\\documentclass{article}"));
        assert!(
            source.contains("\\null"),
            "the blank document still makes a page"
        );

        std::fs::write(&markdown, "mine").unwrap();
        assert!(create_blank(DocumentKind::Markdown, directory.path(), "notes").is_err());
        assert_eq!(std::fs::read_to_string(&markdown).unwrap(), "mine");
    }

    /// What the tree is built from. Build output is not offered, so it cannot
    /// be chosen by accident or on purpose.
    #[test]
    fn the_listing_offers_the_source_and_not_the_build_output() {
        let (_directory, root, _objects) = fixture();
        let offered = listing(&root, &HashSet::new()).unwrap();
        let paths = offered.iter().map(|file| file.path.as_str()).collect::<Vec<_>>();
        assert_eq!(paths, ["figures/plot.pdf", "main.tex", "thesis.cls"]);
    }

    #[test]
    fn only_what_was_chosen_is_kept() {
        let (_directory, root, objects) = fixture();
        let files = keep(&root, &objects, &["main.tex"]);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "main.tex");

        let files = keep(&root, &objects, &["main.tex", "thesis.cls"]);
        let paths = files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>();
        assert_eq!(paths, ["main.tex", "thesis.cls"], "and the figure was not");
    }

    /// The selection arrives from the interface, so a path the walk does not
    /// produce has to be nothing rather than something.
    #[test]
    fn a_path_the_project_does_not_contain_is_ignored() {
        let (directory, root, objects) = fixture();
        std::fs::write(directory.path().join("secrets.txt"), "not yours").unwrap();
        let files = keep(
            &root,
            &objects,
            &["main.tex", "main.aux", "../secrets.txt", "/etc/hosts"],
        );
        let paths = files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>();
        assert_eq!(paths, ["main.tex"], "build output and outsiders alike");
    }

    #[test]
    fn a_selection_without_the_document_is_refused() {
        let (_directory, root, objects) = fixture();
        let chosen = ["thesis.cls".to_owned()];
        assert!(capture(&root, "main.tex", &objects, &HashSet::new(), &chosen).is_err());
    }

    /// One file is a document in the folder that was chosen. Several are a
    /// project, and a project gets a folder.
    #[test]
    fn one_file_lands_beside_its_folder_and_several_land_inside_one() {
        let (directory, root, objects) = fixture();
        let target = directory.path().join("new");
        std::fs::create_dir(&target).unwrap();

        let alone = keep(&root, &objects, &["main.tex"]);
        let document = create(&alone, "main.tex", &objects, &target, "note").unwrap();
        assert_eq!(document, target.join("note.tex"));
        assert!(document.is_file());
        assert!(!target.join("note").exists());

        let whole = keep(&root, &objects, &["main.tex", "thesis.cls", "figures/plot.pdf"]);
        let document = create(&whole, "main.tex", &objects, &target, "paper").unwrap();
        assert_eq!(document, target.join("paper/paper.tex"));
        assert!(document.is_file());
        // Everything else keeps the name the document refers to it by.
        assert!(target.join("paper/thesis.cls").is_file());
        assert!(target.join("paper/figures/plot.pdf").is_file());
        // And nothing of Press's own working state is left behind.
        assert!(!target.join(STAGING).exists());
    }

    #[test]
    fn a_name_already_taken_is_refused_rather_than_written_over() {
        let (directory, root, objects) = fixture();
        let target = directory.path().join("new");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("note.tex"), "mine").unwrap();

        let files = keep(&root, &objects, &["main.tex"]);
        let refused = create(&files, "main.tex", &objects, &target, "note");
        assert!(refused.is_err(), "an occupied name is not written over");
        assert_eq!(
            std::fs::read_to_string(target.join("note.tex")).unwrap(),
            "mine",
            "and what was there is untouched"
        );
    }

    #[test]
    fn a_folder_already_taken_is_refused_too() {
        let (directory, root, objects) = fixture();
        let target = directory.path().join("new");
        std::fs::create_dir_all(target.join("paper")).unwrap();

        let files = keep(&root, &objects, &["main.tex", "thesis.cls", "figures/plot.pdf"]);
        assert!(create(&files, "main.tex", &objects, &target, "paper").is_err());
    }

    /// The whole way round: a project is captured, kept as a template, and a
    /// new document is made from it. What the unit tests above check in pieces,
    /// this checks through the store — which is the seam where a manifest
    /// written by one module is read by another.
    #[test]
    fn a_project_becomes_a_template_and_a_template_becomes_a_project() {
        use crate::{database::Repository, model::{DocumentKind, Engine}};

        let (directory, root, objects) = fixture();
        let repository = Repository::open(&directory.path().join("press.db")).unwrap();

        let files = keep(&root, &objects, &["main.tex", "thesis.cls", "figures/plot.pdf"]);
        let template = repository
            .create_template(
                "Paper",
                DocumentKind::Latex,
                Engine::XeLatex,
                "main.tex",
                &files,
                None,
            )
            .unwrap();

        // Read back the way the command reads it, rather than reusing the
        // manifest that is still in hand.
        let manifest = repository.template_manifest(template.id).unwrap();
        let target = directory.path().join("desk");
        std::fs::create_dir(&target).unwrap();
        let document = create(&manifest, &template.entry, &objects, &target, "quarterly").unwrap();

        assert_eq!(document, target.join("quarterly/quarterly.tex"));
        assert_eq!(
            std::fs::read_to_string(&document).unwrap(),
            "\\documentclass{article}",
            "the document arrives with what it was saved with"
        );
        assert!(target.join("quarterly/thesis.cls").is_file());
        assert!(target.join("quarterly/figures/plot.pdf").is_file());
        // Build output was never captured, so it cannot arrive.
        assert!(!target.join("quarterly/main.aux").exists());
        // And the project it was saved from is exactly as it was.
        assert!(root.join("main.tex").is_file());
        assert!(root.join("main.aux").is_file());
    }

    #[test]
    fn names_that_would_land_somewhere_else_are_refused() {
        for name in ["", "   ", ".hidden", "a/b", "a\\b", "a:b", STAGING] {
            assert!(validate_name(name).is_err(), "{name:?} should be refused");
        }
        assert_eq!(validate_name("  A Paper  ").unwrap(), "A Paper");
    }
}
