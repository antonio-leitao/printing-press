//! Press's own history of a project's source.
//!
//! Deliberately not git. Press never acts as a client for the user's repository,
//! and none of git's real work — branches, merges, remotes, rebases — has any
//! meaning here. What a version needs is: keep these files as they are now, list
//! what was kept, and put them back in a directory so they can be compiled. That
//! is a content-addressed store, and it fits in one file.
//!
//! Files are named by the hash of their contents, so a hundred snapshots of a
//! thesis whose figures never change store those figures once.
//!
//! Nothing here writes to the project folder. It only ever reads.

use std::{
    collections::HashSet,
    fmt::Write as _,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{
    error::{AppError, AppResult},
    files,
};

/// Guards against snapshotting something that is not a paper. Each limit reports
/// what it hit, so the user can narrow the project folder rather than guess.
const MAX_FILES: usize = 5_000;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// Deep enough for any real project layout, shallow enough to stop runaway trees.
const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    /// Project-relative, with forward slashes on every platform.
    pub path: String,
    /// Hex SHA-256 of the contents.
    pub object: String,
    pub byte_size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// Hash of the manifest itself. Two snapshots of identical content share a
    /// revision, and therefore share a cached build.
    pub revision: String,
    pub files: Vec<StoredFile>,
    pub byte_size: i64,
}

/// Reads every source file in the document's directory into the object store and
/// returns the manifest.
///
/// Generated files, editor scratch, other tools' history and the documents of
/// other projects are all skipped, so a snapshot holds what this document needs
/// to compile and nothing else. `foreign` names those other documents, relative
/// to `root`.
pub fn capture(root: &Path, objects: &Path, foreign: &HashSet<String>) -> AppResult<Capture> {
    if !root.is_dir() {
        return Err(AppError::NotFound(format!(
            "{} is no longer a directory",
            root.display()
        )));
    }
    let mut files_seen = Vec::new();
    let mut total = 0_u64;

    for entry in WalkDir::new(root)
        .follow_links(false)
        .max_depth(MAX_DEPTH)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !files::is_ignored_directory(&entry.file_name().to_string_lossy())
        })
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        if !files::belongs_to_project(relative, foreign) {
            continue;
        }

        let size = entry.metadata().map(|data| data.len()).unwrap_or(0);
        if size > MAX_FILE_BYTES {
            return Err(AppError::InvalidInput(format!(
                "{} is larger than the {} MB limit for a snapshot",
                relative.display(),
                MAX_FILE_BYTES / (1024 * 1024)
            )));
        }
        total += size;
        if total > MAX_TOTAL_BYTES {
            return Err(AppError::InvalidInput(format!(
                "this project holds more than {} MB of source; Press will not snapshot it",
                MAX_TOTAL_BYTES / (1024 * 1024)
            )));
        }
        if files_seen.len() == MAX_FILES {
            return Err(AppError::InvalidInput(format!(
                "this project holds more than {MAX_FILES} source files; narrow the folder"
            )));
        }

        let object = store(entry.path(), objects)?;
        files_seen.push(StoredFile {
            path: files::portable(relative),
            object,
            byte_size: size as i64,
        });
    }

    if files_seen.is_empty() {
        return Err(AppError::InvalidInput(
            "there are no source files here to snapshot".into(),
        ));
    }

    files_seen.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Capture {
        revision: manifest_revision(&files_seen),
        byte_size: total as i64,
        files: files_seen,
    })
}

/// Writes a snapshot's files into `destination`, which the caller owns and
/// removes. Copies rather than hard-links: a link would let a build write
/// through into the stored history.
pub fn materialize(files: &[StoredFile], objects: &Path, destination: &Path) -> AppResult<()> {
    for file in files {
        let target = destination.join(&file.path);
        // The manifest holds relative paths built from a directory walk, but the
        // check costs nothing and the store must never write outside its target.
        if !target.starts_with(destination) {
            return Err(AppError::InvalidInput(format!(
                "{} escapes the checkout directory",
                file.path
            )));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let source = object_path(objects, &file.object);
        std::fs::copy(&source, &target).map_err(|error| {
            AppError::Io(std::io::Error::new(
                error.kind(),
                format!("could not restore {}: {error}", file.path),
            ))
        })?;
    }
    Ok(())
}

/// `objects/ab/abcdef…` — a two-character shard so no directory holds a hundred
/// thousand entries.
pub fn object_path(objects: &Path, hash: &str) -> PathBuf {
    let (shard, rest) = hash.split_at(2.min(hash.len()));
    objects.join(shard).join(rest)
}

/// Copies a file into the store under its content hash. A hash that is already
/// present is left alone: identical content is stored once.
fn store(source: &Path, objects: &Path) -> AppResult<String> {
    let hash = hash_file(source)?;
    let destination = object_path(objects, &hash);
    if destination.exists() {
        return Ok(hash);
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Written beside the target and renamed, so a reader never sees a partial
    // object even if Press is killed mid-write.
    let staging = destination.with_extension("incoming");
    std::fs::copy(source, &staging)?;
    match std::fs::rename(&staging, &destination) {
        Ok(()) => Ok(hash),
        Err(error) => {
            let _ = std::fs::remove_file(&staging);
            // Another snapshot may have stored the same content first.
            if destination.exists() {
                Ok(hash)
            } else {
                Err(AppError::Io(error))
            }
        }
    }
}

fn hash_file(path: &Path) -> AppResult<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Hashes the manifest — every path with its content hash — so the revision
/// changes exactly when the tree's content changes.
fn manifest_revision(files: &[StoredFile]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.object.as_bytes());
        hasher.update([0]);
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn project(root: &Path) {
        write(root, "main.tex", "\\documentclass{article}\n");
        write(root, "chapters/one.tex", "First chapter.\n");
        write(root, "references.bib", "@book{a,title={A}}\n");
        // None of the following belongs in a snapshot.
        write(root, "main.aux", "generated");
        write(root, "main.log", "generated");
        write(root, ".git/config", "someone else's history");
        write(root, "node_modules/pkg/index.tex", "vendored");
        write(root, ".DS_Store", "junk");
    }

    #[test]
    fn keeps_source_and_nothing_else() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        let objects = directory.path().join("objects");
        std::fs::create_dir_all(&root).unwrap();
        project(&root);

        let capture = capture(&root, &objects, &HashSet::new()).unwrap();
        let paths = capture
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["chapters/one.tex", "main.tex", "references.bib"]);
        assert!(capture.byte_size > 0);
        assert_eq!(capture.revision.len(), 64);
    }

    #[test]
    fn the_project_folder_is_never_written_to() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        let objects = directory.path().join("objects");
        std::fs::create_dir_all(&root).unwrap();
        project(&root);

        let before = WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
            .map(|entry| entry.path().to_path_buf())
            .collect::<Vec<_>>();
        capture(&root, &objects, &HashSet::new()).unwrap();
        let after = WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
            .map(|entry| entry.path().to_path_buf())
            .collect::<Vec<_>>();
        assert_eq!(before, after, "snapshotting must not touch the source tree");
    }

    #[test]
    fn identical_content_shares_a_revision_and_its_objects() {
        let directory = tempfile::tempdir().unwrap();
        let objects = directory.path().join("objects");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        for root in [&first, &second] {
            std::fs::create_dir_all(root).unwrap();
            write(root, "main.tex", "same bytes\n");
        }

        let one = capture(&first, &objects, &HashSet::new()).unwrap();
        let two = capture(&second, &objects, &HashSet::new()).unwrap();
        assert_eq!(one.revision, two.revision);

        // Stored once, not twice.
        let stored = WalkDir::new(&objects)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .count();
        assert_eq!(stored, 1);
    }

    #[test]
    fn a_changed_file_changes_the_revision() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        let objects = directory.path().join("objects");
        std::fs::create_dir_all(&root).unwrap();
        write(&root, "main.tex", "before\n");
        let before = capture(&root, &objects, &HashSet::new()).unwrap();

        write(&root, "main.tex", "after\n");
        let after = capture(&root, &objects, &HashSet::new()).unwrap();
        assert_ne!(before.revision, after.revision);

        // Renaming a file changes the manifest even though the bytes are the same.
        std::fs::rename(root.join("main.tex"), root.join("paper.tex")).unwrap();
        let renamed = capture(&root, &objects, &HashSet::new()).unwrap();
        assert_ne!(after.revision, renamed.revision);
    }

    #[test]
    fn restores_a_snapshot_exactly() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        let objects = directory.path().join("objects");
        let checkout = directory.path().join("checkout");
        std::fs::create_dir_all(&root).unwrap();
        project(&root);

        let capture = capture(&root, &objects, &HashSet::new()).unwrap();
        // The working tree moves on; the snapshot must not.
        write(&root, "main.tex", "edited after the snapshot\n");

        materialize(&capture.files, &objects, &checkout).unwrap();
        assert_eq!(
            std::fs::read_to_string(checkout.join("main.tex")).unwrap(),
            "\\documentclass{article}\n"
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join("chapters/one.tex")).unwrap(),
            "First chapter.\n"
        );
        // Only what was captured comes back.
        assert!(!checkout.join("main.aux").exists());
        assert!(!checkout.join(".git").exists());
    }

    #[test]
    fn an_empty_project_is_refused_rather_than_stored() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.aux"), "generated").unwrap();
        let error = capture(&root, &directory.path().join("objects"), &HashSet::new()).unwrap_err();
        assert!(error.to_string().contains("no source files"));
    }

    #[test]
    fn an_oversized_file_is_reported_by_name() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.tex"), "ok").unwrap();
        let big = root.join("huge.png");
        let file = File::create(&big).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();

        let error = capture(&root, &directory.path().join("objects"), &HashSet::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("huge.png"), "{error}");
    }

    /// A snapshot of one document does not store the neighbouring documents,
    /// whatever they are written in — otherwise editing the talk would put a new
    /// revision in the essay's history and rebuild it.
    #[test]
    fn a_snapshot_leaves_the_other_projects_documents_out() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("writing");
        let objects = directory.path().join("objects");
        std::fs::create_dir_all(&root).unwrap();
        write(&root, "essay.md", "# An essay\n");
        write(&root, "talk.md", "# A talk, a different project\n");
        write(&root, "poster/poster.tex", "\\documentclass{article}\n");
        write(&root, "figures/plot.png", "not really a png\n");
        write(&root, "references.bib", "@book{a,title={A}}\n");

        let foreign = ["talk.md".to_owned(), "poster/poster.tex".to_owned()]
            .into_iter()
            .collect::<HashSet<_>>();
        let capture = capture(&root, &objects, &foreign).unwrap();
        let paths = capture
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        // The document, and the assets beside it — but neither neighbour.
        assert_eq!(paths, ["essay.md", "figures/plot.png", "references.bib"]);
    }

    #[test]
    fn objects_are_sharded_by_their_first_two_characters() {
        let path = object_path(Path::new("/store"), "abcdef0123");
        assert_eq!(path, Path::new("/store/ab/cdef0123"));
    }
}
