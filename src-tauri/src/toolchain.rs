use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

#[cfg(target_os = "macos")]
use std::{
    fs,
    io::{ErrorKind, Read},
    os::{fd::AsRawFd, unix::ffi::OsStringExt, unix::process::CommandExt},
    process::{Child, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::model::{ToolInfo, ToolchainReport};

/// The path made by the reader's interactive login shell. Finder and the Dock
/// do not give applications that path, but a command field reasonably means
/// the same executable in Press that it does in a terminal.
///
/// It is discovered only after the ordinary application path fails, then kept
/// for the life of the process. Shell startup can be expensive or badly
/// behaved; doing it eagerly would put that cost on readers who never ask for
/// a command outside the usual locations.
static TERMINAL_BINARY_DIRECTORIES: OnceLock<Vec<PathBuf>> = OnceLock::new();

pub fn detect_toolchain() -> ToolchainReport {
    ToolchainReport {
        latexmk: inspect_tool("latexmk"),
        pandoc: inspect_tool("pandoc"),
        neovim: inspect_tool("nvim"),
    }
}

pub fn resolve_executable(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return is_executable(path).then(|| path.to_path_buf());
    }
    // A relative path changes meaning with Press's working directory and can
    // accidentally name a program beside an untrusted document. A command may
    // be either one bare name, searched below, or an absolute path.
    let mut components = path.components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return None;
    }

    let resolved = inherited_binary_directories()
        .chain(common_binary_directories())
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate));
    // macOS applications commonly expose a command-line executable only
    // inside their bundle. A terminal may hide that behind a shim in a user
    // folder, but a GUI application should not depend on being able to read
    // that folder when the installed bundle is already available.
    #[cfg(target_os = "macos")]
    let resolved = resolved.or_else(|| macos_application_executable(path.as_os_str()));
    resolved.or_else(|| {
        terminal_binary_directories()
            .iter()
            .map(|directory| directory.join(name))
            .find(|candidate| is_executable(candidate))
    })
}

/// Finds the executable whose name the reader entered inside an installed
/// macOS application bundle. This is intentionally one level deep under the
/// two conventional application folders: it neither guesses application names
/// nor recursively searches user-controlled document trees.
#[cfg(target_os = "macos")]
fn macos_application_executable(name: &OsStr) -> Option<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        roots.push(home.join("Applications"));
    }
    application_executable_in(name, &roots)
}

#[cfg(target_os = "macos")]
fn application_executable_in(name: &OsStr, roots: &[PathBuf]) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        fs::read_dir(root)
            .ok()?
            .filter_map(Result::ok)
            .find_map(|entry| {
                let bundle = entry.path();
                if bundle.extension() != Some(OsStr::new("app")) {
                    return None;
                }
                let candidate = bundle.join("Contents").join("MacOS").join(name);
                is_executable(&candidate).then_some(candidate)
            })
    })
}

pub fn augmented_path(executable: &Path) -> OsString {
    let mut directories = Vec::new();
    let mut seen = HashSet::new();
    if let Some(parent) = executable.parent() {
        push_directory(&mut directories, &mut seen, parent.to_path_buf());
    }
    // If resolving this executable needed the terminal path, pass the same
    // path to it. This matters for commands such as `alacritty -e nvim`, where
    // Press starts Alacritty and Alacritty must find the next executable.
    if let Some(terminal) = TERMINAL_BINARY_DIRECTORIES.get() {
        for directory in terminal {
            push_directory(&mut directories, &mut seen, directory.clone());
        }
    }
    for directory in common_binary_directories() {
        push_directory(&mut directories, &mut seen, directory);
    }
    for directory in inherited_binary_directories() {
        push_directory(&mut directories, &mut seen, directory);
    }
    // Every entry was checked before it reached this list. An empty fallback
    // is safer than restoring an inherited PATH containing `.` or an empty
    // entry, both of which mean "run a program from the current directory".
    env::join_paths(directories).unwrap_or_default()
}

fn inherited_binary_directories() -> impl Iterator<Item = PathBuf> {
    env::var_os("PATH")
        .map(|path| safe_binary_directories(&path))
        .unwrap_or_default()
        .into_iter()
}

fn push_directory(directories: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, directory: PathBuf) {
    if safe_binary_directory(&directory) && seen.insert(directory.clone()) {
        directories.push(directory);
    }
}

/// Keeps PATH lookup deterministic and prevents the two shell conventions that
/// search the current directory: a relative entry and an empty entry.
fn safe_binary_directories(path: &OsStr) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut seen = HashSet::new();
    for directory in env::split_paths(path) {
        push_directory(&mut directories, &mut seen, directory);
    }
    directories
}

fn safe_binary_directory(path: &Path) -> bool {
    path.is_absolute()
        && path.is_dir()
        && !path.to_string_lossy().chars().any(char::is_control)
        // A directory containing the platform's PATH separator cannot be put
        // back into PATH without changing where one entry ends and the next
        // begins. `join_paths` is the standard library's exact check for it.
        && env::join_paths(std::iter::once(path.as_os_str())).is_ok()
}

fn terminal_binary_directories() -> &'static [PathBuf] {
    TERMINAL_BINARY_DIRECTORIES
        .get_or_init(discover_terminal_binary_directories)
        .as_slice()
}

#[cfg(not(target_os = "macos"))]
fn discover_terminal_binary_directories() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "macos")]
const PATH_PROBE_COMMAND: &str =
    "/usr/bin/printf '\\036'; /usr/bin/printenv PATH; /usr/bin/printf '\\037'";
#[cfg(target_os = "macos")]
const PATH_PROBE_START: u8 = 0x1e;
#[cfg(target_os = "macos")]
const PATH_PROBE_END: u8 = 0x1f;
#[cfg(target_os = "macos")]
const PATH_PROBE_LIMIT: usize = 64 * 1024;
#[cfg(target_os = "macos")]
const PATH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "macos")]
const PATH_PROBE_DRAIN: Duration = Duration::from_millis(50);

/// Runs one fixed command in the reader's shell. The configured editor command
/// and document paths never enter the shell, so they cannot become syntax.
/// Interactive login mode matches a terminal on macOS, where PATH additions
/// commonly live in either login or interactive startup files.
#[cfg(target_os = "macos")]
fn discover_terminal_binary_directories() -> Vec<PathBuf> {
    let shell = env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && is_executable(path))
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    probe_terminal_path(&shell, PATH_PROBE_TIMEOUT).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn probe_terminal_path(shell: &Path, timeout: Duration) -> Option<Vec<PathBuf>> {
    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c", PATH_PROBE_COMMAND])
        .env("PRESS_PATH_PROBE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Startup files occasionally launch children. A separate process group
        // lets a timeout stop the probe and those children without touching the
        // application or any existing terminal session.
        .process_group(0);

    let mut child = command.spawn().ok()?;
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_probe(&mut child);
            return None;
        }
    };
    if !set_nonblocking(&stdout) {
        terminate_probe(&mut child);
        return None;
    }

    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let status = loop {
        if read_probe_output(&mut stdout, &mut output).is_err() {
            terminate_probe(&mut child);
            return None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => {
                terminate_probe(&mut child);
                return None;
            }
        }
        if Instant::now() >= deadline {
            terminate_probe(&mut child);
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        return None;
    }

    // A background process from a startup file may inherit stdout and keep the
    // pipe open after the shell exits. Drain only long enough to collect bytes
    // already written; never wait for EOF indefinitely.
    let drain_deadline = Instant::now() + PATH_PROBE_DRAIN;
    while !output.contains(&PATH_PROBE_END) && Instant::now() < drain_deadline {
        match read_probe_output(&mut stdout, &mut output) {
            Ok(true) => break,
            Ok(false) => thread::sleep(Duration::from_millis(2)),
            Err(()) => return None,
        }
    }
    parse_terminal_path(&output)
}

#[cfg(target_os = "macos")]
fn set_nonblocking(stdout: &ChildStdout) -> bool {
    let descriptor = stdout.as_raw_fd();
    // SAFETY: fcntl only reads and updates flags on the live pipe descriptor
    // owned by `stdout`; no pointer or borrowed memory crosses the call.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    flags >= 0
        // SAFETY: the descriptor is still owned by `stdout`, and F_SETFL with
        // O_NONBLOCK changes only how subsequent reads behave.
        && unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0
}

#[cfg(target_os = "macos")]
fn read_probe_output(stdout: &mut ChildStdout, output: &mut Vec<u8>) -> Result<bool, ()> {
    let mut buffer = [0_u8; 4096];
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(read) => {
                if output.len().saturating_add(read) > PATH_PROBE_LIMIT {
                    return Err(());
                }
                output.extend_from_slice(&buffer[..read]);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(false),
            Err(_) => return Err(()),
        }
    }
}

#[cfg(target_os = "macos")]
fn terminate_probe(child: &mut Child) {
    let process_group = -(child.id() as libc::pid_t);
    // SAFETY: the probe was placed in a new process group whose id is its pid.
    // A negative pid asks kill to address that group and no other process.
    unsafe {
        libc::kill(process_group, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "macos")]
fn parse_terminal_path(output: &[u8]) -> Option<Vec<PathBuf>> {
    let start = output.iter().rposition(|byte| *byte == PATH_PROBE_START)? + 1;
    let end = output[start..]
        .iter()
        .position(|byte| *byte == PATH_PROBE_END)?
        + start;
    let mut path = output[start..end].to_vec();
    while matches!(path.last(), Some(b'\n' | b'\r')) {
        path.pop();
    }
    // Output inside the markers should contain only one PATH value. Reject it
    // altogether if an asynchronous startup message was interleaved.
    if path.iter().any(u8::is_ascii_control) {
        return None;
    }
    Some(safe_binary_directories(&OsString::from_vec(path)))
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
    use std::fs;

    #[cfg(target_os = "macos")]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn always_resolves_standard_shell() {
        assert!(resolve_executable("sh").is_some());
    }

    #[test]
    fn relative_executable_paths_are_never_resolved() {
        assert!(resolve_executable("./sh").is_none());
        assert!(resolve_executable("../bin/sh").is_none());
        assert!(resolve_executable(".").is_none());
    }

    #[test]
    fn unsafe_path_entries_are_discarded() {
        let temporary = tempfile::tempdir().unwrap();
        let first = temporary.path().join("first");
        let second = temporary.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let path = env::join_paths([
            first.as_os_str(),
            OsStr::new(""),
            OsStr::new("relative"),
            second.as_os_str(),
            first.as_os_str(),
        ])
        .unwrap();

        assert_eq!(safe_binary_directories(&path), [first, second]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn marked_shell_output_yields_only_safe_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let first = temporary.path().join("first");
        let second = temporary.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let path = env::join_paths([
            first.as_os_str(),
            OsStr::new("relative"),
            OsStr::new(""),
            second.as_os_str(),
            first.as_os_str(),
        ])
        .unwrap();
        let mut output = b"startup chatter\n\x1e".to_vec();
        output.extend_from_slice(path.as_encoded_bytes());
        output.extend_from_slice(b"\n\x1fexit chatter\n");

        assert_eq!(parse_terminal_path(&output), Some(vec![first, second]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_shell_path_probe_is_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        let shell = temporary.path().join("slow-shell");
        fs::write(&shell, "#!/bin/sh\n/bin/sleep 5\n").unwrap();
        let mut permissions = fs::metadata(&shell).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&shell, permissions).unwrap();

        let started = Instant::now();
        assert!(probe_terminal_path(&shell, Duration::from_millis(50)).is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_shell_path_probe_reads_only_the_marked_value() {
        let temporary = tempfile::tempdir().unwrap();
        let shell = temporary.path().join("test-shell");
        fs::write(
            &shell,
            "#!/bin/sh\n/usr/bin/printf 'noise\\036/usr/bin\\n\\037more noise\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&shell).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&shell, permissions).unwrap();

        assert_eq!(
            probe_terminal_path(&shell, Duration::from_secs(1)),
            Some(vec![PathBuf::from("/usr/bin")])
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn resolves_an_executable_inside_an_installed_application_bundle() {
        let temporary = tempfile::tempdir().unwrap();
        let applications = temporary.path().join("Applications");
        let executable = applications
            .join("Example Terminal.app")
            .join("Contents/MacOS/example-terminal");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();

        assert_eq!(
            application_executable_in(OsStr::new("example-terminal"), &[applications]),
            Some(executable)
        );
    }
}
