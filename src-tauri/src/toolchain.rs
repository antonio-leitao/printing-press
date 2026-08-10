use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::model::{ToolInfo, ToolchainReport};

pub fn detect_toolchain() -> ToolchainReport {
    ToolchainReport {
        latexmk: inspect_tool("latexmk"),
        pandoc: inspect_tool("pandoc"),
        neovim: inspect_tool("nvim"),
    }
}

pub fn resolve_executable(name: &str) -> Option<PathBuf> {
    if name.contains(std::path::MAIN_SEPARATOR) {
        let path = PathBuf::from(name);
        return is_executable(&path).then_some(path);
    }

    env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .chain(common_binary_directories())
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

pub fn augmented_path(executable: &Path) -> OsString {
    let mut directories = Vec::new();
    if let Some(parent) = executable.parent() {
        directories.push(parent.to_path_buf());
    }
    directories.extend(common_binary_directories());
    if let Some(path) = env::var_os("PATH") {
        directories.extend(env::split_paths(&path));
    }
    env::join_paths(directories).unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default())
}

/// Whether a tool is installed, and where.
///
/// Whether, and no more than that. Each of these used to be asked for its
/// version, which is three process launches — about 280ms, most of it Perl
/// starting up under `latexmk -v` — on every path Press is asked to resolve.
/// That is the slowest thing between `:Press` in the editor and a document on
/// screen, and nothing has ever read a version: the interface asks only whether
/// the tool is there.
///
/// A binary that is present but broken now reports itself present and fails when
/// it is run, which is the better error of the two — latexmk's own complaint
/// says what is wrong with the installation, and "latexmk was not found" does not.
fn inspect_tool(name: &str) -> ToolInfo {
    let path = resolve_executable(name);
    ToolInfo {
        available: path.is_some(),
        path: path.and_then(|path| path.to_str().map(ToOwned::to_owned)),
    }
}

fn common_binary_directories() -> impl Iterator<Item = PathBuf> {
    [
        "/Library/TeX/texbin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
    ]
    .into_iter()
    .map(PathBuf::from)
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_resolves_standard_shell() {
        assert!(resolve_executable("sh").is_some());
    }
}
