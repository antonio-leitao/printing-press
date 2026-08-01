use std::{
    collections::VecDeque,
    fs::File,
    io::{Read as StdRead, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicI32, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use notify::{Event, EventKind, RecursiveMode, Watcher};
use regex::Regex;
use tauri::{AppHandle, Emitter};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::{Mutex, mpsc, oneshot},
};

use crate::{
    database::Repository,
    error::{AppError, AppResult},
    model::ProjectSummary,
    toolchain::{augmented_path, resolve_executable},
};

const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const DEBOUNCE: Duration = Duration::from_millis(250);
static FILE_LINE_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)([^\r\n]+\.tex):(\d+):\s*(.+)").unwrap());

pub struct BuildManager {
    repository: Arc<Repository>,
    artifact_root: PathBuf,
    work_root: PathBuf,
    current: Mutex<Option<SessionHandle>>,
    active_pid: Arc<AtomicI32>,
}

struct SessionHandle {
    project_id: i64,
    sender: mpsc::UnboundedSender<SessionMessage>,
}

enum SessionMessage {
    Trigger,
    WatcherError(String),
    Stop(oneshot::Sender<()>),
}

struct BuildResult {
    published_pdf: PathBuf,
}

enum BuildRun {
    Finished {
        result: AppResult<BuildResult>,
        duration_ms: i64,
        dirty: bool,
    },
    Superseded,
    Stopped,
}

impl BuildManager {
    pub fn new(repository: Arc<Repository>, artifact_root: PathBuf, work_root: PathBuf) -> Self {
        Self {
            repository,
            artifact_root,
            work_root,
            current: Mutex::new(None),
            active_pid: Arc::new(AtomicI32::new(0)),
        }
    }

    pub async fn activate(&self, app: AppHandle, project: ProjectSummary) -> AppResult<()> {
        self.stop().await;
        self.repository.touch_project(project.id)?;

        let (sender, receiver) = mpsc::unbounded_channel();
        let handle = SessionHandle {
            project_id: project.id,
            sender: sender.clone(),
        };
        *self.current.lock().await = Some(handle);

        let repository = Arc::clone(&self.repository);
        let artifact_root = self.artifact_root.clone();
        let work_root = self.work_root.clone();
        let active_pid = Arc::clone(&self.active_pid);
        tauri::async_runtime::spawn(async move {
            session_loop(
                app,
                repository,
                project,
                artifact_root,
                work_root,
                active_pid,
                sender.clone(),
                receiver,
            )
            .await;
        });
        let _ = self
            .current
            .lock()
            .await
            .as_ref()
            .and_then(|session| session.sender.send(SessionMessage::Trigger).ok());
        Ok(())
    }

    pub async fn rebuild(&self, project_id: i64) -> AppResult<()> {
        let guard = self.current.lock().await;
        let session = guard.as_ref().ok_or_else(|| {
            AppError::InvalidInput("open the project before requesting a rebuild".into())
        })?;
        if session.project_id != project_id {
            return Err(AppError::InvalidInput(
                "the requested project is not currently open".into(),
            ));
        }
        session
            .sender
            .send(SessionMessage::Trigger)
            .map_err(|_| AppError::Task("the active build session has stopped".into()))
    }

    pub async fn stop(&self) {
        let session = self.current.lock().await.take();
        let Some(session) = session else {
            return;
        };
        terminate_process_group(self.active_pid.load(Ordering::SeqCst), libc::SIGTERM);
        let (acknowledge, receiver) = oneshot::channel();
        if session
            .sender
            .send(SessionMessage::Stop(acknowledge))
            .is_ok()
        {
            let _ = tokio::time::timeout(Duration::from_secs(3), receiver).await;
        }
        let remaining = self.active_pid.swap(0, Ordering::SeqCst);
        terminate_process_group(remaining, libc::SIGKILL);
    }

    pub fn shutdown_now(&self) {
        let pid = self.active_pid.swap(0, Ordering::SeqCst);
        terminate_process_group(pid, libc::SIGTERM);
        terminate_process_group(pid, libc::SIGKILL);
    }

    pub fn log_path(&self, project_id: i64) -> PathBuf {
        self.work_root
            .join(project_id.to_string())
            .join("last-build.log")
    }
}

#[allow(clippy::too_many_arguments)]
async fn session_loop(
    app: AppHandle,
    repository: Arc<Repository>,
    project: ProjectSummary,
    artifact_root: PathBuf,
    work_root: PathBuf,
    active_pid: Arc<AtomicI32>,
    sender: mpsc::UnboundedSender<SessionMessage>,
    mut receiver: mpsc::UnboundedReceiver<SessionMessage>,
) {
    let watcher_sender = sender.clone();
    let mut watcher =
        match notify::recommended_watcher(move |result: notify::Result<Event>| match result {
            Ok(event) if relevant_event(&event) => {
                let _ = watcher_sender.send(SessionMessage::Trigger);
            }
            Ok(_) => {}
            Err(error) => {
                let _ = watcher_sender.send(SessionMessage::WatcherError(error.to_string()));
            }
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                emit_session_error(
                    &app,
                    &repository,
                    project.id,
                    format!("File watcher failed: {error}"),
                );
                return;
            }
        };
    if let Err(error) = watcher.watch(&project.root(), RecursiveMode::Recursive) {
        emit_session_error(
            &app,
            &repository,
            project.id,
            format!("Could not watch project files: {error}"),
        );
        return;
    }

    let mut pending = false;
    loop {
        let should_wait_for_trigger = !pending;
        if should_wait_for_trigger {
            match receiver.recv().await {
                Some(SessionMessage::Trigger) => {}
                Some(SessionMessage::WatcherError(error)) => {
                    emit_session_error(
                        &app,
                        &repository,
                        project.id,
                        format!("File watcher failed: {error}"),
                    );
                    continue;
                }
                Some(SessionMessage::Stop(acknowledge)) => {
                    let _ = acknowledge.send(());
                    return;
                }
                None => return,
            }
        }

        let debounce = tokio::time::sleep(DEBOUNCE);
        tokio::pin!(debounce);
        loop {
            tokio::select! {
                _ = &mut debounce => break,
                message = receiver.recv() => match message {
                    Some(SessionMessage::Trigger) => {
                        debounce.as_mut().reset(tokio::time::Instant::now() + DEBOUNCE);
                    }
                    Some(SessionMessage::WatcherError(error)) => {
                        emit_session_error(
                            &app,
                            &repository,
                            project.id,
                            format!("File watcher failed: {error}"),
                        );
                        continue;
                    }
                    Some(SessionMessage::Stop(acknowledge)) => {
                        let _ = acknowledge.send(());
                        return;
                    }
                    None => return,
                }
            }
        }
        match repository.record_build_started(project.id) {
            Ok(summary) => emit_project(&app, &summary),
            Err(error) => {
                eprintln!("Press could not record build start: {error}");
                return;
            }
        }

        match execute_build(
            &project,
            &artifact_root,
            &work_root,
            &active_pid,
            &mut receiver,
        )
        .await
        {
            BuildRun::Stopped => return,
            BuildRun::Superseded => pending = true,
            BuildRun::Finished {
                result,
                duration_ms,
                dirty,
            } => {
                let summary = match result {
                    Ok(result) => {
                        let old_pdf = repository.pdf_path(project.id).ok();
                        match repository.record_build_success(
                            project.id,
                            duration_ms,
                            &result.published_pdf,
                        ) {
                            Ok(summary) => {
                                if let Some(old) =
                                    old_pdf.filter(|path| path != &result.published_pdf)
                                {
                                    let _ = std::fs::remove_file(old);
                                }
                                summary
                            }
                            Err(error) => {
                                eprintln!("Press could not record successful build: {error}");
                                return;
                            }
                        }
                    }
                    Err(error) => match repository.record_build_failure(
                        project.id,
                        duration_ms,
                        &error.to_string(),
                    ) {
                        Ok(summary) => summary,
                        Err(database_error) => {
                            eprintln!("Press could not record build failure: {database_error}");
                            return;
                        }
                    },
                };
                emit_project(&app, &summary);
                pending = dirty;
            }
        }
    }
}

async fn execute_build(
    project: &ProjectSummary,
    artifact_root: &Path,
    work_root: &Path,
    active_pid: &AtomicI32,
    receiver: &mut mpsc::UnboundedReceiver<SessionMessage>,
) -> BuildRun {
    let started = Instant::now();
    let result =
        prepare_and_run_build(project, artifact_root, work_root, active_pid, receiver).await;
    let duration_ms = started.elapsed().as_millis() as i64;
    match result {
        Ok(RunOutcome::Stopped) => BuildRun::Stopped,
        Ok(RunOutcome::Superseded) => BuildRun::Superseded,
        Ok(RunOutcome::Completed { pdf, dirty }) => BuildRun::Finished {
            result: Ok(BuildResult { published_pdf: pdf }),
            duration_ms,
            dirty,
        },
        Err((error, dirty)) => BuildRun::Finished {
            result: Err(AppError::Build(error)),
            duration_ms,
            dirty,
        },
    }
}

enum RunOutcome {
    Completed { pdf: PathBuf, dirty: bool },
    Superseded,
    Stopped,
}

async fn prepare_and_run_build(
    project: &ProjectSummary,
    artifact_root: &Path,
    work_root: &Path,
    active_pid: &AtomicI32,
    receiver: &mut mpsc::UnboundedReceiver<SessionMessage>,
) -> Result<RunOutcome, (String, bool)> {
    let latexmk = resolve_executable("latexmk").ok_or_else(|| {
        (
            "latexmk is not available in the app environment".into(),
            false,
        )
    })?;
    let work_directory = work_root.join(project.id.to_string()).join("work");
    let log_path = work_root
        .join(project.id.to_string())
        .join("last-build.log");
    let artifact_directory = artifact_root.join(project.id.to_string()).join("artifacts");
    tokio::fs::create_dir_all(&work_directory)
        .await
        .map_err(|error| (format!("could not create build directory: {error}"), false))?;
    tokio::fs::create_dir_all(&artifact_directory)
        .await
        .map_err(|error| {
            (
                format!("could not create artifact directory: {error}"),
                false,
            )
        })?;

    let mut command = Command::new(&latexmk);
    command.current_dir(project.working_path());
    command.env("PATH", augmented_path(&latexmk));
    command.arg(match project.engine.as_str() {
        "xelatex" => "-pdfxe",
        "lualatex" => "-pdflua",
        _ => "-pdf",
    });
    command.args([
        "-interaction=nonstopmode",
        "-file-line-error",
        "-synctex=1",
        "-recorder",
    ]);
    if let Some(configuration) = latexmk_configuration(&project.root(), &project.working_path()) {
        command.arg("-r").arg(configuration);
    }
    command.arg(format!("-outdir={}", work_directory.display()));
    command.arg(project.main_path());
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }

    let mut child = command
        .spawn()
        .map_err(|error| (format!("could not start latexmk: {error}"), false))?;
    let pid = child.id().unwrap_or_default() as i32;
    active_pid.store(pid, Ordering::SeqCst);
    let stdout_task = child
        .stdout
        .take()
        .map(|stdout| tokio::spawn(read_limited(stdout)));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_limited(stderr)));
    let mut dirty = false;

    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            message = receiver.recv() => match message {
                Some(SessionMessage::Trigger) => dirty = true,
                Some(SessionMessage::WatcherError(error)) => {
                    terminate_child(&mut child, pid).await;
                    active_pid.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst).ok();
                    return Err((format!("File watcher failed: {error}"), false));
                }
                Some(SessionMessage::Stop(acknowledge)) => {
                    terminate_child(&mut child, pid).await;
                    active_pid.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst).ok();
                    let _ = acknowledge.send(());
                    return Ok(RunOutcome::Stopped);
                }
                None => {
                    terminate_child(&mut child, pid).await;
                    active_pid.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst).ok();
                    return Ok(RunOutcome::Stopped);
                }
            }
        }
    };
    active_pid
        .compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst)
        .ok();

    let stdout = join_reader(stdout_task).await;
    let stderr = join_reader(stderr_task).await;
    let mut combined = stdout;
    if !combined.is_empty() && !stderr.is_empty() {
        combined.push(b'\n');
    }
    combined.extend(stderr);
    if combined.len() > OUTPUT_LIMIT {
        combined.drain(..combined.len() - OUTPUT_LIMIT);
    }
    let _ = tokio::fs::write(&log_path, &combined).await;
    let output = String::from_utf8_lossy(&combined);

    loop {
        match receiver.try_recv() {
            Ok(SessionMessage::Trigger) => dirty = true,
            Ok(SessionMessage::WatcherError(error)) => {
                return Err((format!("File watcher failed: {error}"), false));
            }
            Ok(SessionMessage::Stop(acknowledge)) => {
                let _ = acknowledge.send(());
                return Ok(RunOutcome::Stopped);
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => return Ok(RunOutcome::Stopped),
        }
    }

    let status = status.map_err(|error| (format!("could not wait for latexmk: {error}"), dirty))?;
    if dirty {
        return Ok(RunOutcome::Superseded);
    }
    if !status.success() {
        let message = first_build_error(&output)
            .unwrap_or_else(|| format!("latexmk exited with {}", status.code().unwrap_or(-1)));
        return Err((message, dirty));
    }

    let generated = find_generated_pdf(&work_directory, &project.main_path())
        .map_err(|error| (error, dirty))?;
    verify_pdf(&generated).map_err(|error| (error, dirty))?;
    let artifact_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let destination = artifact_directory.join(format!("build-{artifact_id}.pdf"));
    let staging = artifact_directory.join(format!("build-{artifact_id}.next"));
    tokio::fs::copy(&generated, &staging)
        .await
        .map_err(|error| (format!("could not stage successful PDF: {error}"), dirty))?;
    tokio::fs::rename(&staging, &destination)
        .await
        .map_err(|error| (format!("could not publish successful PDF: {error}"), dirty))?;
    Ok(RunOutcome::Completed {
        pdf: destination,
        dirty,
    })
}

async fn read_limited<R>(mut reader: R) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut output = VecDeque::with_capacity(OUTPUT_LIMIT);
    let mut chunk = [0_u8; 8192];
    while let Ok(count) = reader.read(&mut chunk).await {
        if count == 0 {
            break;
        }
        output.extend(chunk[..count].iter().copied());
        if output.len() > OUTPUT_LIMIT {
            output.drain(..output.len() - OUTPUT_LIMIT);
        }
    }
    output.into()
}

async fn join_reader(task: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    match task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    }
}

fn find_generated_pdf(directory: &Path, main: &Path) -> Result<PathBuf, String> {
    let expected = directory.join(
        main.file_stem()
            .map(|stem| {
                let mut value = stem.to_os_string();
                value.push(".pdf");
                value
            })
            .ok_or_else(|| "main file has no filename".to_owned())?,
    );
    if expected.is_file() {
        return Ok(expected);
    }
    let mut candidates = std::fs::read_dir(directory)
        .map_err(|error| format!("could not inspect build output: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH)
    });
    candidates
        .pop()
        .ok_or_else(|| "latexmk succeeded but produced no PDF".into())
}

fn verify_pdf(path: &Path) -> Result<(), String> {
    let mut file =
        File::open(path).map_err(|error| format!("could not read generated PDF: {error}"))?;
    let length = file
        .metadata()
        .map_err(|error| format!("could not inspect generated PDF: {error}"))?
        .len();
    if length < 10 {
        return Err("latexmk produced a file that is not a readable PDF".into());
    }
    let mut header = [0_u8; 5];
    file.read_exact(&mut header)
        .map_err(|error| format!("could not read generated PDF header: {error}"))?;
    if &header != b"%PDF-" {
        return Err("latexmk produced a file that is not a readable PDF".into());
    }
    let tail_length = length.min(2048) as usize;
    file.seek(SeekFrom::End(-(tail_length as i64)))
        .map_err(|error| format!("could not seek generated PDF: {error}"))?;
    let mut tail = vec![0_u8; tail_length];
    file.read_exact(&mut tail)
        .map_err(|error| format!("could not read generated PDF trailer: {error}"))?;
    if !tail.windows(5).any(|window| window == b"%%EOF") {
        return Err("latexmk produced an incomplete PDF without an end marker".into());
    }
    Ok(())
}

fn latexmk_configuration(root: &Path, working: &Path) -> Option<PathBuf> {
    if root == working {
        return None;
    }
    [root.join(".latexmkrc"), root.join("latexmkrc")]
        .into_iter()
        .find(|path| path.is_file())
}

fn first_build_error(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(capture) = FILE_LINE_ERROR.captures(trimmed) {
            return Some(format!(
                "{}:{}: {}",
                capture.get(1)?.as_str(),
                capture.get(2)?.as_str(),
                capture.get(3)?.as_str().trim()
            ));
        }
        let lower = trimmed.to_ascii_lowercase();
        if trimmed.starts_with("! ")
            || lower.contains("latex error")
            || lower.contains("undefined control sequence")
            || lower.contains("emergency stop")
            || lower.contains("fatal error")
        {
            return Some(trimmed.chars().take(400).collect());
        }
    }
    None
}

fn relevant_event(event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| !ignored_watch_path(path))
}

fn ignored_watch_path(path: &Path) -> bool {
    const IGNORED_DIRECTORIES: &[&str] = &[
        ".git",
        ".hg",
        ".svn",
        ".cache",
        "node_modules",
        "target",
        "build",
        "out",
        "dist",
    ];
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|value| IGNORED_DIRECTORIES.contains(&value))
    }) {
        return true;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name == ".DS_Store"
        || name.starts_with(".#")
        || name.ends_with('~')
        || name.starts_with(".nfs")
        || name.ends_with(".synctex.gz")
        || name.ends_with(".run.xml")
    {
        return true;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "aux"
                    | "acn"
                    | "acr"
                    | "alg"
                    | "bbl"
                    | "bcf"
                    | "blg"
                    | "fdb_latexmk"
                    | "fls"
                    | "glg"
                    | "glo"
                    | "gls"
                    | "idx"
                    | "ilg"
                    | "ind"
                    | "lof"
                    | "log"
                    | "lot"
                    | "nav"
                    | "out"
                    | "snm"
                    | "swo"
                    | "swp"
                    | "synctex"
                    | "tmp"
                    | "toc"
            )
        })
}

fn emit_project(app: &AppHandle, project: &ProjectSummary) {
    let _ = app.emit("project-updated", project);
}

fn emit_session_error(app: &AppHandle, repository: &Repository, id: i64, message: String) {
    if let Ok(project) = repository.record_build_failure(id, 0, &message) {
        emit_project(app, &project);
    }
}

fn terminate_process_group(pid: i32, signal: i32) {
    if pid <= 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-pid, signal);
    }
    #[cfg(windows)]
    {
        let _ = (pid, signal);
    }
}

async fn terminate_child(child: &mut tokio::process::Child, pid: i32) {
    terminate_process_group(pid, libc::SIGTERM);
    if tokio::time::timeout(Duration::from_millis(1200), child.wait())
        .await
        .is_err()
    {
        terminate_process_group(pid, libc::SIGKILL);
        let _ = child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_useful_latex_errors() {
        let output = "noise\n./chapter.tex:42: Undefined control sequence.\nmore";
        assert_eq!(
            first_build_error(output).as_deref(),
            Some("./chapter.tex:42: Undefined control sequence.")
        );
    }

    #[test]
    fn verifies_real_pdf_headers() {
        let directory = tempfile::tempdir().unwrap();
        let good = directory.path().join("good.pdf");
        let bad = directory.path().join("bad.pdf");
        std::fs::write(&good, b"%PDF-1.7\nbody\n%%EOF\n").unwrap();
        std::fs::write(&bad, b"not pdf").unwrap();
        assert!(verify_pdf(&good).is_ok());
        assert!(verify_pdf(&bad).is_err());
    }

    #[test]
    fn recognizes_source_changes_but_not_aux_files() {
        let source = Event {
            kind: EventKind::Any,
            paths: vec![PathBuf::from("chapter.tex")],
            attrs: Default::default(),
        };
        let aux = Event {
            kind: EventKind::Any,
            paths: vec![PathBuf::from("main.aux")],
            attrs: Default::default(),
        };
        let data = Event {
            kind: EventKind::Any,
            paths: vec![PathBuf::from("figures/data.csv")],
            attrs: Default::default(),
        };
        let git = Event {
            kind: EventKind::Any,
            paths: vec![PathBuf::from(".git/index")],
            attrs: Default::default(),
        };
        assert!(relevant_event(&source));
        assert!(relevant_event(&data));
        assert!(!relevant_event(&aux));
        assert!(!relevant_event(&git));
    }

    #[test]
    fn compiles_and_publishes_a_real_latex_document_when_available() {
        if resolve_executable("latexmk").is_none() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("source");
        let artifacts = directory.path().join("artifacts");
        let work = directory.path().join("work-cache");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("main.tex"),
            "\\documentclass{article}\n\\begin{document}\n\\input{chapter}\n\\end{document}\n",
        )
        .unwrap();
        std::fs::write(root.join("chapter.tex"), "Press works.\n").unwrap();
        let project = ProjectSummary {
            id: 1,
            name: "Fixture".into(),
            root_path: root.to_string_lossy().into_owned(),
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
        };
        let active_pid = AtomicI32::new(0);
        let (sender, mut receiver) = mpsc::unbounded_channel();

        let outcome = tauri::async_runtime::block_on(prepare_and_run_build(
            &project,
            &artifacts,
            &work,
            &active_pid,
            &mut receiver,
        ))
        .unwrap();

        let RunOutcome::Completed { pdf, dirty } = outcome else {
            panic!("build unexpectedly stopped");
        };
        assert!(!dirty);
        assert!(pdf.is_file());
        verify_pdf(&pdf).unwrap();
        assert!(work.join("1/last-build.log").is_file());

        std::fs::write(root.join("chapter.tex"), "Press rebuilds included files.\n").unwrap();
        let rebuilt = tauri::async_runtime::block_on(prepare_and_run_build(
            &project,
            &artifacts,
            &work,
            &active_pid,
            &mut receiver,
        ))
        .unwrap();
        let RunOutcome::Completed {
            pdf: rebuilt_pdf, ..
        } = rebuilt
        else {
            panic!("incremental build unexpectedly stopped");
        };
        assert_ne!(pdf, rebuilt_pdf);
        verify_pdf(&rebuilt_pdf).unwrap();

        sender.send(SessionMessage::Trigger).unwrap();
        let superseded = tauri::async_runtime::block_on(prepare_and_run_build(
            &project,
            &artifacts,
            &work,
            &active_pid,
            &mut receiver,
        ))
        .unwrap();
        assert!(matches!(superseded, RunOutcome::Superseded));
        let published_count = std::fs::read_dir(artifacts.join("1/artifacts"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|value| value == "pdf"))
            .count();
        assert_eq!(published_count, 2);
    }
}
