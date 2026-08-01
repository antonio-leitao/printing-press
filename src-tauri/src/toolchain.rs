use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use crate::model::{ToolInfo, ToolchainReport};

pub fn detect_toolchain() -> ToolchainReport {
    ToolchainReport {
        latexmk: inspect_tool("latexmk", &["-v"]),
        neovim: inspect_tool("nvim", &["--version"]),
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

fn inspect_tool(name: &str, version_args: &[&str]) -> ToolInfo {
    let Some(path) = resolve_executable(name) else {
        return ToolInfo {
            available: false,
            path: None,
            version: None,
        };
    };
    let output = Command::new(&path).args(version_args).output();
    let Ok(output) = output else {
        return ToolInfo {
            available: false,
            path: path.to_str().map(ToOwned::to_owned),
            version: None,
        };
    };
    if !output.status.success() {
        return ToolInfo {
            available: false,
            path: path.to_str().map(ToOwned::to_owned),
            version: None,
        };
    }
    let version = {
        let text = if output.stdout.is_empty() {
            String::from_utf8_lossy(&output.stderr)
        } else {
            String::from_utf8_lossy(&output.stdout)
        };
        text.lines()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_owned())
    };
    ToolInfo {
        available: true,
        path: path.to_str().map(ToOwned::to_owned),
        version,
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
