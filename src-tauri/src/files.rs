//! What counts as part of a LaTeX project.
//!
//! Two questions share one answer. The watcher asks "should this change start a
//! build?" and the snapshot store asks "is this worth keeping a copy of?". Both
//! mean "is this a source file the author wrote, or something a tool produced?",
//! so the lists live here rather than drifting apart in two modules.

use std::path::Path;

/// Directories that never hold project source. `.git` matters most: the user's
/// own history is never read, let alone copied.
const IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".cache",
    ".venv",
    "node_modules",
    "target",
    "build",
    "out",
    "dist",
    "__pycache__",
];

/// Extensions TeX and its friends produce. Deliberately excludes `pdf`: a PDF in
/// a project folder is almost always a figure, and Press writes its own output
/// elsewhere, so there is no feedback loop to guard against.
const GENERATED_EXTENSIONS: &[&str] = &[
    "aux",
    "acn",
    "acr",
    "alg",
    "bbl",
    "bcf",
    "blg",
    "fdb_latexmk",
    "fls",
    "glg",
    "glo",
    "gls",
    "idx",
    "ilg",
    "ind",
    "lof",
    "log",
    "lot",
    "nav",
    "out",
    "snm",
    "swo",
    "swp",
    "synctex",
    "tmp",
    "toc",
    "xdv",
];

pub fn is_ignored_directory(name: &str) -> bool {
    IGNORED_DIRECTORIES.contains(&name.to_ascii_lowercase().as_str())
}

/// True when any component of the path is a directory Press never looks inside.
pub fn is_in_ignored_directory(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(is_ignored_directory)
    })
}

/// True for build output and editor scratch files: everything that is a
/// by-product rather than something the author typed.
pub fn is_generated(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    // Editor scratch: Vim swap and backup files, Vim's atomic-write probe,
    // Emacs lock links, NFS silly-renames, and macOS folder metadata.
    if name == ".DS_Store"
        || name == "4913"
        || name.starts_with(".#")
        || name.starts_with(".nfs")
        || name.ends_with('~')
        || name.ends_with(".synctex.gz")
        || name.ends_with(".run.xml")
    {
        return true;
    }

    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            GENERATED_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// Whether a path is project source worth watching and worth snapshotting.
pub fn is_project_source(path: &Path) -> bool {
    !is_in_ignored_directory(path) && !is_generated(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn keeps_what_the_author_wrote() {
        for path in [
            "main.tex",
            "chapters/one.tex",
            "references.bib",
            "figures/plot.pdf",
            "figures/photo.png",
            "data/results.csv",
            ".latexmkrc",
            "styles/thesis.cls",
        ] {
            assert!(
                is_project_source(&PathBuf::from(path)),
                "{path} is source and should be kept"
            );
        }
    }

    #[test]
    fn drops_what_a_tool_produced() {
        for path in [
            "main.aux",
            "main.log",
            "main.toc",
            "main.fdb_latexmk",
            "main.synctex.gz",
            "main.run.xml",
            ".main.tex.swp",
            ".#main.tex",
            "main.tex~",
            "4913",
            ".DS_Store",
        ] {
            assert!(
                !is_project_source(&PathBuf::from(path)),
                "{path} is generated and should be dropped"
            );
        }
    }

    #[test]
    fn never_descends_into_someone_elses_history() {
        assert!(!is_project_source(&PathBuf::from(".git/index")));
        assert!(!is_project_source(&PathBuf::from("node_modules/pkg/a.tex")));
        assert!(!is_project_source(&PathBuf::from("deep/target/debug/x.tex")));
        assert!(is_ignored_directory(".GIT"));
    }
}
