use std::{
    env,
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::{
    error::{AppError, AppResult},
    model::{EditorLaunchResult, ProjectSummary},
    toolchain::resolve_executable,
};

pub fn launch(project: &ProjectSummary) -> AppResult<EditorLaunchResult> {
    let nvim = resolve_executable("nvim")
        .ok_or_else(|| AppError::ToolUnavailable("Neovim was not found.".into()))?;
    let socket = socket_path(project);
    let main = project.main_path();

    if socket.exists() {
        let status = Command::new(&nvim)
            .arg("--server")
            .arg(&socket)
            .arg("--remote")
            .arg(&main)
            .status();
        if status.is_ok_and(|status| status.success()) {
            return Ok(EditorLaunchResult {
                status: "connected".into(),
                socket_path: socket.to_string_lossy().into_owned(),
                message: "Reused the running Neovim for this project.".into(),
            });
        }
        std::fs::remove_file(&socket).map_err(|error| {
            AppError::Io(std::io::Error::new(
                error.kind(),
                format!("could not remove stale Neovim socket: {error}"),
            ))
        })?;
    }

    let alacritty = resolve_alacritty()
        .ok_or_else(|| AppError::ToolUnavailable("Alacritty was not found.".into()))?;
    let mut command = Command::new(alacritty);
    command
        .arg("--working-directory")
        .arg(project.working_path())
        .arg("-e")
        .arg(nvim)
        .arg("--listen")
        .arg(&socket)
        .arg(main)
        .current_dir(project.working_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()?;

    Ok(EditorLaunchResult {
        status: "launched".into(),
        socket_path: socket.to_string_lossy().into_owned(),
        message: "Launched Neovim in Alacritty.".into(),
    })
}

pub fn status(project: &ProjectSummary) -> String {
    let socket = socket_path(project);
    if !socket.exists() {
        return "closed".into();
    }
    let Some(nvim) = resolve_executable("nvim") else {
        return "unavailable".into();
    };
    let alive = Command::new(nvim)
        .arg("--server")
        .arg(&socket)
        .args(["--remote-expr", "1"])
        .output()
        .is_ok_and(|output| output.status.success());
    if alive {
        "connected".into()
    } else {
        "closed".into()
    }
}

fn resolve_alacritty() -> Option<PathBuf> {
    resolve_executable("alacritty")
        .or_else(|| resolve_executable("/Applications/Alacritty.app/Contents/MacOS/alacritty"))
}

fn socket_path(project: &ProjectSummary) -> PathBuf {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in project.root_path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    env::temp_dir().join(format!("press-nvim-{hash:016x}.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root_path: &str) -> ProjectSummary {
        ProjectSummary {
            id: 1,
            name: "Test".into(),
            root_path: root_path.into(),
            main_file: "main.tex".into(),
            working_directory: ".".into(),
            engine: "pdflatex".into(),
            build_status: "never".into(),
            last_build_at: None,
            last_build_duration_ms: None,
            last_error: None,
            artifact_revision: 0,
            has_pdf: false,
            path_available: true,
        }
    }

    #[test]
    fn sockets_are_short_and_project_specific() {
        let first = socket_path(&project("/tmp/one"));
        let second = socket_path(&project("/tmp/two"));
        assert_ne!(first, second);
        assert!(first.to_string_lossy().len() < 100);
    }
}
