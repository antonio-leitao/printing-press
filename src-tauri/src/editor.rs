use std::{
    env,
    io::ErrorKind,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    error::{AppError, AppResult},
    model::{EditorLaunchResult, Project},
    toolchain::resolve_executable,
};

/// Terminals Press knows how to open, in the order they are tried. Each takes
/// its working directory and its command differently.
const TERMINALS: &[Terminal] = &[
    Terminal {
        command: "alacritty",
        fallback: Some("/Applications/Alacritty.app/Contents/MacOS/alacritty"),
        directory: DirectoryArgument::Separate("--working-directory"),
        execute: &["-e"],
    },
    Terminal {
        command: "kitty",
        fallback: Some("/Applications/kitty.app/Contents/MacOS/kitty"),
        directory: DirectoryArgument::Separate("--directory"),
        execute: &[],
    },
    Terminal {
        command: "ghostty",
        fallback: Some("/Applications/Ghostty.app/Contents/MacOS/ghostty"),
        directory: DirectoryArgument::Joined("--working-directory="),
        execute: &["-e"],
    },
    Terminal {
        command: "wezterm",
        fallback: Some("/Applications/WezTerm.app/Contents/MacOS/wezterm"),
        directory: DirectoryArgument::Prefixed(&["start"], "--cwd"),
        execute: &["--"],
    },
];

struct Terminal {
    command: &'static str,
    fallback: Option<&'static str>,
    directory: DirectoryArgument,
    execute: &'static [&'static str],
}

enum DirectoryArgument {
    /// `--working-directory <dir>`
    Separate(&'static str),
    /// `--working-directory=<dir>`
    Joined(&'static str),
    /// Leading subcommand, then `--cwd <dir>`
    Prefixed(&'static [&'static str], &'static str),
}

pub fn launch(project: &Project) -> AppResult<EditorLaunchResult> {
    let nvim = resolve_executable("nvim").ok_or_else(|| {
        AppError::ToolUnavailable("Neovim was not found. Install nvim or add it to PATH.".into())
    })?;
    let socket = socket_path(project);
    let document = project.document();

    // A running Neovim for this project just gets told to open the file.
    if socket_is_live(&socket) {
        let opened = Command::new(&nvim)
            .arg("--server")
            .arg(&socket)
            .arg("--remote")
            .arg(&document)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if opened.is_ok_and(|status| status.success()) {
            return Ok(EditorLaunchResult {
                status: "connected".into(),
                socket_path: socket.to_string_lossy().into_owned(),
                message: "Opened the file in the Neovim already running for this project.".into(),
            });
        }
    }
    // Either nothing is listening or it stopped answering; the file is stale.
    remove_stale_socket(&socket)?;

    let (terminal, path) = resolve_terminal().ok_or_else(|| {
        AppError::ToolUnavailable(
            "No supported terminal was found. Press can open Alacritty, kitty, Ghostty or WezTerm."
                .into(),
        )
    })?;

    let working = project.directory();
    let mut command = Command::new(path);
    match terminal.directory {
        DirectoryArgument::Separate(flag) => {
            command.arg(flag).arg(&working);
        }
        DirectoryArgument::Joined(flag) => {
            command.arg(format!("{flag}{}", working.display()));
        }
        DirectoryArgument::Prefixed(leading, flag) => {
            command.args(leading).arg(flag).arg(&working);
        }
    }
    command
        .args(terminal.execute)
        .arg(nvim)
        .arg("--listen")
        .arg(&socket)
        .arg(document)
        .current_dir(&working)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own group, so quitting Press never takes the editor with it.
        command.process_group(0);
    }
    command.spawn()?;

    Ok(EditorLaunchResult {
        status: "launched".into(),
        socket_path: socket.to_string_lossy().into_owned(),
        message: format!("Launched Neovim in {}.", terminal.command),
    })
}

/// Whether an editor is listening for this project.
///
/// Connecting to the socket is proof enough and costs a syscall; the previous
/// version spawned a whole `nvim --server` process for every poll.
pub fn status(project: &Project) -> &'static str {
    if socket_is_live(&socket_path(project)) {
        "connected"
    } else {
        "closed"
    }
}

/// A successful connect proves something is listening. `ConnectionRefused` is the
/// signature of a socket file that outlived the process that created it.
fn socket_is_live(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}

fn remove_stale_socket(socket: &Path) -> AppResult<()> {
    match std::fs::remove_file(socket) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::Io(std::io::Error::new(
            error.kind(),
            format!("could not remove a stale Neovim socket: {error}"),
        ))),
    }
}

fn resolve_terminal() -> Option<(&'static Terminal, PathBuf)> {
    TERMINALS.iter().find_map(|terminal| {
        let path = resolve_executable(terminal.command)
            .or_else(|| terminal.fallback.and_then(resolve_executable))?;
        Some((terminal, path))
    })
}

/// One stable socket per project, so reopening a project finds the editor that
/// is already running for it. Keyed on the document, so two papers sharing a
/// folder get an editor each.
fn socket_path(project: &Project) -> PathBuf {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in project.document_path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    env::temp_dir().join(format!("press-nvim-{hash:016x}.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Engine;

    fn project(document_path: &str) -> Project {
        Project {
            id: 1,
            name: "Test".into(),
            document_path: document_path.into(),
            engine: Engine::PdfLatex,
            created_at: 0,
            last_opened_at: 0,
        }
    }

    #[test]
    fn sockets_are_short_stable_and_document_specific() {
        let first = socket_path(&project("/tmp/one/main.tex"));
        let second = socket_path(&project("/tmp/two/main.tex"));
        assert_ne!(first, second);
        assert_eq!(first, socket_path(&project("/tmp/one/main.tex")));
        // Two documents sharing a folder are two projects, and get an editor each.
        assert_ne!(
            socket_path(&project("/tmp/one/paper.tex")),
            socket_path(&project("/tmp/one/supplementary.tex"))
        );
        // Unix socket paths are limited to about 104 bytes on macOS.
        assert!(first.to_string_lossy().len() < 100);
    }

    #[test]
    fn a_missing_socket_reads_as_closed() {
        let directory = tempfile::tempdir().unwrap();
        assert!(!socket_is_live(&directory.path().join("absent.sock")));
        let document = directory.path().join("main.tex");
        assert_eq!(status(&project(document.to_str().unwrap())), "closed");
    }

    #[test]
    fn an_orphaned_socket_file_reads_as_closed_and_can_be_cleared() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("stale.sock");
        // A plain file where a socket should be: what a crashed editor leaves.
        std::fs::write(&socket, b"").unwrap();
        assert!(!socket_is_live(&socket));
        remove_stale_socket(&socket).unwrap();
        assert!(!socket.exists());
        remove_stale_socket(&socket).unwrap();
    }

    #[test]
    fn a_listening_socket_reads_as_connected() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("live.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(socket_is_live(&socket));
        drop(listener);
    }
}
