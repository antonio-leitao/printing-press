//! Running one build.
//!
//! Everything here is parameterized on a [`PreparedSource`] rather than on a
//! project, so compiling a version out of the history is the same code path as
//! compiling the working tree.
//!
//! Two rules this module enforces, both from hard experience:
//!
//! * A successful build always publishes. Whether it has already been
//!   superseded by another save is the queue's business; throwing away a
//!   finished PDF means a document that saves faster than it compiles never
//!   updates at all.
//! * The child's pid is only ever signalled while its handle is still alive, so
//!   a reaped pid can never be recycled and signalled by mistake.

use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read as _, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::watch,
};

use crate::{
    diagnostics::{self, ProgressParser, ProgressSnapshot},
    error::{AppError, AppResult},
    model::{Diagnostic, DocumentKind, Project, Severity},
    sources::PreparedSource,
    toolchain::{augmented_path, resolve_executable},
};

/// The build log kept for the user; older output is discarded first.
const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
/// How long a terminated build gets to exit before it is killed outright.
const TERMINATE_GRACE: std::time::Duration = std::time::Duration::from_millis(1200);

pub type ProgressSink = Arc<dyn Fn(ProgressSnapshot) + Send + Sync>;

/// The stage a markdown build starts in. The webview matches on it, so it is
/// named here rather than spelled out at the call site.
pub const PANDOC_STAGE: &str = "pandoc";

/// Trips once, for everyone watching. Used for hard cancellation only: closing a
/// project or quitting. A build that has merely been superseded still finishes
/// and publishes.
#[derive(Clone)]
pub struct Cancel {
    receiver: watch::Receiver<bool>,
}

pub struct CancelHandle {
    sender: watch::Sender<bool>,
}

impl CancelHandle {
    pub fn new() -> (Self, Cancel) {
        let (sender, receiver) = watch::channel(false);
        (Self { sender }, Cancel { receiver })
    }

    pub fn cancel(&self) {
        let _ = self.sender.send(true);
    }
}

impl Cancel {
    pub fn is_cancelled(&self) -> bool {
        *self.receiver.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.receiver.clone();
        // An error means the handle was dropped, which can only happen once the
        // manager has given up on this build; treat it as cancellation.
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}

/// Live pids, for the synchronous kill at application exit. Entries are removed
/// before the child is reaped, so a pid in here is always a running process.
#[derive(Default)]
pub struct PidRegistry {
    inner: Mutex<HashMap<u64, i32>>,
}

impl PidRegistry {
    fn register(&self, build_id: u64, pid: i32) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(build_id, pid);
        }
    }

    fn unregister(&self, build_id: u64) -> Option<i32> {
        self.inner.lock().ok()?.remove(&build_id)
    }

    /// Signals every live build. Only safe to call when the process is going
    /// away: it does not reap the children.
    pub fn terminate_all(&self) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        for (_, pid) in guard.drain() {
            terminate_process_group(pid, libc::SIGTERM);
            terminate_process_group(pid, libc::SIGKILL);
        }
    }
}

pub struct BuildInputs<'a> {
    pub build_id: u64,
    pub project: &'a Project,
    pub source: &'a PreparedSource,
    /// latexmk's `-outdir`: reusable auxiliary files, never inside the project.
    pub work_directory: PathBuf,
    /// Combined stdout and stderr, kept for the user.
    pub log_path: PathBuf,
    /// Where published PDFs live.
    pub artifact_directory: PathBuf,
}

pub struct BuildProduct {
    pub pdf_path: PathBuf,
    pub page_count: Option<i64>,
    pub byte_size: i64,
}

pub enum BuildOutcome {
    /// The document compiled. `diagnostics` may still hold warnings.
    Succeeded {
        product: BuildProduct,
        diagnostics: Vec<Diagnostic>,
    },
    /// The document did not compile. This is about the document, not about Press.
    Failed {
        diagnostics: Vec<Diagnostic>,
        summary: String,
    },
    Cancelled,
}

/// Compiles one version of one project.
///
/// `Err` means Press could not run the build at all (no latexmk, unwritable
/// cache). `Ok(Failed)` means the document has errors. The two are reported
/// differently, so they are not collapsed.
pub async fn run(
    inputs: BuildInputs<'_>,
    cancel: Cancel,
    pids: Arc<PidRegistry>,
    progress: ProgressSink,
) -> AppResult<BuildOutcome> {
    let latexmk = resolve_executable("latexmk").ok_or_else(|| {
        AppError::ToolUnavailable(
            "latexmk was not found. Install a TeX distribution or add latexmk to PATH.".into(),
        )
    })?;
    tokio::fs::create_dir_all(&inputs.work_directory).await?;
    tokio::fs::create_dir_all(&inputs.artifact_directory).await?;
    if let Some(parent) = inputs.log_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if cancel.is_cancelled() {
        return Ok(BuildOutcome::Cancelled);
    }

    let job_name = inputs.project.job_name();

    // Markdown reaches latexmk through pandoc. Running the two stages here,
    // rather than letting `pandoc --pdf-engine` drive the whole thing, is what
    // keeps latexmk's incremental cache: pandoc invokes TeX from scratch every
    // time and throws the auxiliary files away.
    let latex_input = match inputs.project.kind() {
        DocumentKind::Latex => PathBuf::from(&inputs.source.file_name),
        DocumentKind::Markdown => {
            // pandoc says nothing a parser could use, and it runs before
            // latexmk has said anything at all. Announcing the stage is what
            // keeps a markdown build from looking stalled while it converts.
            progress(ProgressSnapshot {
                stage: PANDOC_STAGE.to_owned(),
                pass: None,
                page: None,
            });
            match convert_markdown(&inputs, &job_name).await {
                Ok(path) => path,
                Err(summary) => {
                    return Ok(BuildOutcome::Failed {
                        diagnostics: vec![Diagnostic {
                            file: Some(inputs.source.file_name.clone()),
                            line: None,
                            severity: Severity::Error,
                            message: summary.clone(),
                        }],
                        summary,
                    });
                }
            }
        }
    };
    if cancel.is_cancelled() {
        return Ok(BuildOutcome::Cancelled);
    }

    let mut command = Command::new(&latexmk);
    command.current_dir(&inputs.source.directory);
    command.env("PATH", augmented_path(&latexmk));
    // TeX wraps its log at `max_print_line` columns, which splits file paths
    // across lines and makes the log unparseable. Widening it is what makes
    // structured diagnostics possible at all.
    command.env("max_print_line", "1000");
    command.env("error_line", "254");
    command.env("half_error_line", "238");
    command.arg(inputs.project.engine.latexmk_flag());
    command.args([
        "-interaction=nonstopmode",
        "-file-line-error",
        "-synctex=1",
        "-recorder",
    ]);
    // A `.latexmkrc` beside the document is loaded by latexmk itself, because it
    // is the directory latexmk runs in. Nothing to pass, and nothing that can
    // execute except what the user was warned about before adding the document.
    //
    // Owning the job name makes the output path deterministic, so a stale PDF
    // from an earlier build can never be mistaken for this one's.
    command.arg(format!("-jobname={job_name}"));
    command.arg(format!("-outdir={}", inputs.work_directory.display()));
    // For markdown this is an absolute path into the work directory, while the
    // command still runs from the source directory, so `\includegraphics` and
    // friends resolve against the folder the author wrote in.
    command.arg(&latex_input);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }

    let mut child = command
        .spawn()
        .map_err(|error| AppError::Build(format!("could not start latexmk: {error}")))?;
    let pid = child.id().unwrap_or_default() as i32;
    pids.register(inputs.build_id, pid);

    let pump = Arc::new(Mutex::new(Pump::new()));
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(tokio::spawn(drain(
            stdout,
            Arc::clone(&pump),
            Arc::clone(&progress),
        )));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(tokio::spawn(drain(
            stderr,
            Arc::clone(&pump),
            Arc::clone(&progress),
        )));
    }

    let status = tokio::select! {
        status = child.wait() => {
            pids.unregister(inputs.build_id);
            status
        }
        () = cancel.cancelled() => {
            // The child handle is still alive here, so this pid is certainly ours.
            terminate_process_group(pid, libc::SIGTERM);
            if tokio::time::timeout(TERMINATE_GRACE, child.wait()).await.is_err() {
                terminate_process_group(pid, libc::SIGKILL);
                let _ = child.wait().await;
            }
            pids.unregister(inputs.build_id);
            for reader in readers {
                let _ = reader.await;
            }
            return Ok(BuildOutcome::Cancelled);
        }
    };

    for reader in readers {
        let _ = reader.await;
    }
    let output = pump
        .lock()
        .map(|pump| pump.bytes())
        .unwrap_or_default();
    let _ = tokio::fs::write(&inputs.log_path, &output).await;
    let terminal_output = String::from_utf8_lossy(&output).into_owned();

    // The .log file is authoritative for TeX's own errors; latexmk's failures
    // only ever appear on the terminal.
    let tex_log = tokio::fs::read(inputs.work_directory.join(format!("{job_name}.log")))
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let analysis = diagnostics::analyze_log(&tex_log, &inputs.source.directory);
    let mut all = analysis.diagnostics;
    all.extend(diagnostics::latexmk_failures(&terminal_output));
    if inputs.project.kind() == DocumentKind::Markdown {
        attribute_to_source(&mut all, &latex_input, &inputs.source.file_name);
    }

    let status = status.map_err(|error| AppError::Build(format!("latexmk did not finish: {error}")))?;
    if !status.success() {
        let summary = diagnostics::summarize(&all).unwrap_or_else(|| {
            format!(
                "latexmk exited with status {}",
                status.code().unwrap_or(-1)
            )
        });
        return Ok(BuildOutcome::Failed {
            diagnostics: all,
            summary,
        });
    }

    let generated = inputs.work_directory.join(format!("{job_name}.pdf"));
    if !generated.is_file() {
        let summary = diagnostics::summarize(&all).unwrap_or_else(|| {
            "latexmk reported success but produced no PDF".to_owned()
        });
        return Ok(BuildOutcome::Failed {
            diagnostics: all,
            summary,
        });
    }
    if let Err(reason) = verify_pdf(&generated) {
        return Ok(BuildOutcome::Failed {
            diagnostics: all,
            summary: reason,
        });
    }

    let product = publish(
        &generated,
        &inputs.work_directory,
        &job_name,
        &inputs.artifact_directory,
        analysis.page_count,
    )
    .await?;
    Ok(BuildOutcome::Succeeded {
        product,
        diagnostics: all,
    })
}

/// Moves diagnostics off the generated LaTeX and onto the markdown the author
/// actually wrote.
///
/// The line numbers have to go with them. They refer to pandoc's output, and
/// there is no map back to the markdown, so a line number here would point
/// somewhere plausible and wrong — worse than none at all.
fn attribute_to_source(diagnostics: &mut [Diagnostic], generated: &Path, document: &str) {
    let generated = generated.to_string_lossy();
    let generated_name = generated
        .rsplit('/')
        .next()
        .unwrap_or(generated.as_ref())
        .to_owned();
    for diagnostic in diagnostics {
        let Some(file) = diagnostic.file.as_deref() else {
            continue;
        };
        // The log may name it absolutely or by bare filename depending on how
        // TeX happened to print it.
        if file == generated || file.ends_with(&generated_name) {
            diagnostic.file = Some(document.to_owned());
            diagnostic.line = None;
        }
    }
}

/// Converts markdown to LaTeX with pandoc, returning the generated file.
///
/// `Err` carries a message fit to show above the PDF: pandoc's own complaint,
/// which for markdown is usually about the document rather than about pandoc.
async fn convert_markdown(inputs: &BuildInputs<'_>, job_name: &str) -> Result<PathBuf, String> {
    let pandoc = resolve_executable("pandoc")
        .ok_or_else(|| "pandoc was not found. Install pandoc to compile markdown.".to_owned())?;
    let source = inputs.source.document();
    let generated = inputs.work_directory.join(format!("{job_name}.tex"));
    // Written beside the target first: latexmk must never see a half-written
    // file, and comparing before replacing is what preserves its cache.
    let staging = inputs.work_directory.join(format!("{job_name}.tex.next"));

    // pandoc reads a marked copy rather than the document itself, so that the
    // LaTeX it writes says which line of the markdown each block came from.
    // The copy sits in the work directory; pandoc still runs in the source
    // directory, so images and includes resolve where the author put them.
    let marked = inputs.work_directory.join(format!("{job_name}.marked.md"));
    let read = tokio::fs::read_to_string(&source)
        .await
        .map_err(|error| format!("could not read {}: {error}", inputs.source.file_name))?;
    let input = match tokio::fs::write(&marked, crate::anchors::mark(&read)).await {
        Ok(()) => marked.clone(),
        // Marking is an aid, not a requirement. A document that cannot be
        // marked is still a document that should compile.
        Err(_) => source.clone(),
    };

    let run = |input: PathBuf| {
        let mut command = Command::new(&pandoc);
        command.current_dir(&inputs.source.directory);
        command.env("PATH", augmented_path(&pandoc));
        command.args(["--from", "markdown", "--to", "latex"]);
        // Standalone gives a full document rather than a fragment, and applies
        // the YAML frontmatter — title, author, documentclass, geometry —
        // through pandoc's default template.
        command.arg("--standalone");
        command.arg("--output").arg(&staging);
        command.arg(input);
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command.kill_on_drop(true);
        command
    };

    let mut output = run(input.clone())
        .output()
        .await
        .map_err(|error| format!("could not start pandoc: {error}"))?;
    // pandoc counts lines in what it was given, so a complaint about the marked
    // copy would name lines the author cannot find. Asking again with the
    // document itself costs a second run only when something is already wrong.
    if !output.status.success() && input != source {
        output = run(source.clone())
            .output()
            .await
            .map_err(|error| format!("could not start pandoc: {error}"))?;
    }
    if !output.status.success() {
        let _ = tokio::fs::remove_file(&staging).await;
        let complaint = String::from_utf8_lossy(&output.stderr);
        let first = complaint
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("pandoc could not convert this document");
        return Err(first.chars().take(400).collect());
    }

    // Only replace the generated file when its contents actually changed.
    // Rewriting it every build would make latexmk redo work it had cached.
    let fresh = tokio::fs::read(&staging)
        .await
        .map_err(|error| format!("could not read pandoc's output: {error}"))?;
    let unchanged = tokio::fs::read(&generated)
        .await
        .is_ok_and(|existing| existing == fresh);
    if unchanged {
        let _ = tokio::fs::remove_file(&staging).await;
    } else {
        tokio::fs::rename(&staging, &generated)
            .await
            .map_err(|error| format!("could not store pandoc's output: {error}"))?;
    }
    Ok(generated)
}

/// Copies the PDF into Press-managed storage under a fresh name, staged and then
/// renamed so a reader never sees a half-written file.
///
/// What SyncTeX wrote travels with it. The work directory is scratch space that
/// the next build overwrites, so a sync file left there would describe a PDF
/// nobody is looking at any more; beside the artifact it stays true for as long
/// as the artifact does. `synctex` finds it by the PDF's own name, which is why
/// it is copied under the same stem.
async fn publish(
    generated: &Path,
    work_directory: &Path,
    job_name: &str,
    artifact_directory: &Path,
    page_count: Option<i64>,
) -> AppResult<BuildProduct> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let destination = artifact_directory.join(format!("build-{stamp}.pdf"));
    let staging = artifact_directory.join(format!("build-{stamp}.next"));
    let byte_size = tokio::fs::copy(generated, &staging).await.map_err(|error| {
        AppError::Build(format!("could not stage the built PDF: {error}"))
    })?;
    tokio::fs::rename(&staging, &destination).await.map_err(|error| {
        AppError::Build(format!("could not publish the built PDF: {error}"))
    })?;

    // Best effort throughout: a build with no sync data is a build whose PDF
    // cannot be clicked through to its source, which is worth nothing beside
    // failing the build itself.
    for extension in ["synctex.gz", "synctex"] {
        let sync = work_directory.join(format!("{job_name}.{extension}"));
        if tokio::fs::copy(
            &sync,
            artifact_directory.join(format!("build-{stamp}.{extension}")),
        )
        .await
        .is_ok()
        {
            break;
        }
    }
    // For markdown, SyncTeX can only name pandoc's output. The anchors in it
    // are what carry an answer back to the markdown the author wrote.
    if let Ok(latex) = tokio::fs::read_to_string(work_directory.join(format!("{job_name}.tex"))).await
    {
        let anchors = crate::anchors::collect(&latex);
        if !anchors.is_empty() {
            let _ = tokio::fs::write(
                artifact_directory.join(format!("build-{stamp}.lines")),
                crate::anchors::encode(&anchors),
            )
            .await;
        }
    }

    Ok(BuildProduct {
        pdf_path: destination,
        page_count,
        byte_size: byte_size as i64,
    })
}

fn verify_pdf(path: &Path) -> Result<(), String> {
    let mut file =
        File::open(path).map_err(|error| format!("could not read the built PDF: {error}"))?;
    let length = file
        .metadata()
        .map_err(|error| format!("could not inspect the built PDF: {error}"))?
        .len();
    if length < 10 {
        return Err("latexmk produced a file that is not a readable PDF".into());
    }
    let mut header = [0_u8; 5];
    file.read_exact(&mut header)
        .map_err(|error| format!("could not read the built PDF header: {error}"))?;
    if &header != b"%PDF-" {
        return Err("latexmk produced a file that is not a readable PDF".into());
    }
    let tail_length = length.min(2048) as usize;
    file.seek(SeekFrom::End(-(tail_length as i64)))
        .map_err(|error| format!("could not seek the built PDF: {error}"))?;
    let mut tail = vec![0_u8; tail_length];
    file.read_exact(&mut tail)
        .map_err(|error| format!("could not read the built PDF trailer: {error}"))?;
    if !tail.windows(5).any(|window| window == b"%%EOF") {
        return Err("latexmk produced an incomplete PDF without an end marker".into());
    }
    Ok(())
}

pub fn terminate_process_group(pid: i32, signal: i32) {
    if pid <= 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        // Negative pid signals the whole group, which is where latexmk's own
        // children (pdflatex, biber) live.
        libc::kill(-pid, signal);
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
    }
}

async fn drain<R>(mut reader: R, pump: Arc<Mutex<Pump>>, progress: ProgressSink)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut chunk = [0_u8; 8192];
    while let Ok(count) = reader.read(&mut chunk).await {
        if count == 0 {
            break;
        }
        let snapshots = match pump.lock() {
            Ok(mut pump) => pump.push(&chunk[..count]),
            Err(_) => break,
        };
        // Emitted outside the lock: the sink reaches the webview.
        for snapshot in snapshots {
            progress(snapshot);
        }
    }
}

/// Accumulates output for the log file while watching it go past for progress.
struct Pump {
    bytes: VecDeque<u8>,
    pending: String,
    parser: ProgressParser,
}

impl Pump {
    fn new() -> Self {
        Self {
            bytes: VecDeque::new(),
            pending: String::new(),
            parser: ProgressParser::default(),
        }
    }

    fn push(&mut self, data: &[u8]) -> Vec<ProgressSnapshot> {
        self.bytes.extend(data.iter().copied());
        if self.bytes.len() > OUTPUT_LIMIT {
            let excess = self.bytes.len() - OUTPUT_LIMIT;
            self.bytes.drain(..excess);
        }

        // Lossy is safe here: only the progress parser reads this text, while the
        // log file is written from the untouched bytes.
        self.pending.push_str(&String::from_utf8_lossy(data));
        let mut snapshots = Vec::new();
        while let Some(index) = self.pending.find('\n') {
            let line = self.pending.drain(..=index).collect::<String>();
            if let Some(snapshot) = self.parser.observe(line.trim_end()) {
                snapshots.push(snapshot);
            }
        }
        // TeX emits page markers without a trailing newline, so the incomplete
        // tail is inspected too. The parser only reports changes, so re-reading
        // the same tail costs nothing.
        if !self.pending.is_empty()
            && let Some(snapshot) = self.parser.observe_partial(&self.pending.clone())
        {
            snapshots.push(snapshot);
        }
        if self.pending.len() > 64 * 1024 {
            self.pending.clear();
        }
        snapshots
    }

    fn bytes(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Engine, SourceRef};

    fn pdf_bytes() -> &'static [u8] {
        b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\ntrailer\n%%EOF\n"
    }

    #[test]
    fn accepts_complete_pdfs_and_rejects_the_rest() {
        let directory = tempfile::tempdir().unwrap();
        let good = directory.path().join("good.pdf");
        let truncated = directory.path().join("truncated.pdf");
        let wrong = directory.path().join("wrong.pdf");
        std::fs::write(&good, pdf_bytes()).unwrap();
        std::fs::write(&truncated, b"%PDF-1.7\nbody without an end marker\n").unwrap();
        std::fs::write(&wrong, b"not a pdf at all").unwrap();

        assert!(verify_pdf(&good).is_ok());
        assert!(verify_pdf(&truncated).is_err());
        assert!(verify_pdf(&wrong).is_err());
    }

    #[test]
    fn the_pump_reports_progress_across_chunk_boundaries() {
        let mut pump = Pump::new();
        // A page marker split across two reads must still be seen, once.
        let first = pump.push(b"Run number 1 of rule 'pdflatex'\n[1] [2");
        assert!(first.iter().any(|snapshot| snapshot.pass == Some(1)));
        assert!(first.iter().any(|snapshot| snapshot.page == Some(1)));

        let second = pump.push(b"] [3]\n");
        assert_eq!(second.last().unwrap().page, Some(3));

        assert_eq!(
            String::from_utf8(pump.bytes()).unwrap(),
            "Run number 1 of rule 'pdflatex'\n[1] [2] [3]\n"
        );
    }

    #[test]
    fn the_pump_keeps_only_the_tail_of_a_very_long_build() {
        let mut pump = Pump::new();
        let noise = vec![b'x'; OUTPUT_LIMIT];
        pump.push(&noise);
        pump.push(b"final line\n");
        let bytes = pump.bytes();
        assert_eq!(bytes.len(), OUTPUT_LIMIT);
        assert!(bytes.ends_with(b"final line\n"));
    }

    #[test]
    fn a_cancel_handle_trips_every_watcher() {
        let (handle, cancel) = CancelHandle::new();
        let second = cancel.clone();
        assert!(!cancel.is_cancelled());
        handle.cancel();
        assert!(cancel.is_cancelled());
        assert!(second.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_resolves_when_the_handle_is_dropped() {
        let (handle, cancel) = CancelHandle::new();
        drop(handle);
        // A manager that has gone away must not leave a build waiting forever.
        tokio::time::timeout(std::time::Duration::from_secs(1), cancel.cancelled())
            .await
            .expect("cancellation resolved");
    }

    fn fixture_project(document: &Path) -> Project {
        Project {
            id: 1,
            name: "Fixture".into(),
            document_path: document.to_string_lossy().into_owned(),
            engine: Engine::PdfLatex,
            pinned: false,
            created_at: 0,
            last_opened_at: 0,
        }
    }

    /// Exercises the real toolchain when there is one; skipped otherwise so the
    /// suite still runs on a machine without TeX.
    #[tokio::test]
    async fn compiles_publishes_and_reports_a_real_document() {
        if resolve_executable("latexmk").is_none() {
            eprintln!("skipping: latexmk is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("source");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("main.tex"),
            "\\documentclass{article}\n\\begin{document}\n\\input{chapter}\n\\end{document}\n",
        )
        .unwrap();
        std::fs::write(root.join("chapter.tex"), "Press works.\n").unwrap();

        let project = fixture_project(&root.join("main.tex"));
        let store = tempfile::tempdir().unwrap();
        let repository = crate::database::Repository::open(&store.path().join("press.db")).unwrap();
        let source = crate::sources::prepare(
            &project,
            &SourceRef::Worktree,
            &repository,
            store.path(),
        )
        .unwrap();
        let (_handle, cancel) = CancelHandle::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let sink: ProgressSink = Arc::new(move |snapshot: ProgressSnapshot| {
            recorder.lock().unwrap().push(snapshot);
        });

        let outcome = run(
            BuildInputs {
                build_id: 1,
                project: &project,
                source: &source,
                work_directory: directory.path().join("work"),
                log_path: directory.path().join("work/last-build.log"),
                artifact_directory: directory.path().join("artifacts"),
            },
            cancel,
            Arc::new(PidRegistry::default()),
            sink,
        )
        .await
        .unwrap();

        let BuildOutcome::Succeeded { product, .. } = outcome else {
            panic!("a valid document should compile");
        };
        assert!(product.pdf_path.is_file());
        assert_eq!(product.page_count, Some(1));
        assert!(product.byte_size > 0);
        verify_pdf(&product.pdf_path).unwrap();
        assert!(directory.path().join("work/last-build.log").is_file());
        // The job name is ours, so the output path never depends on the source name.
        assert!(directory.path().join("work/main.pdf").is_file());
        assert!(!seen.lock().unwrap().is_empty(), "progress was reported");
    }

    /// The two-stage pipeline, end to end: pandoc reads the markdown, latexmk
    /// builds what it wrote, and a PDF comes out.
    #[tokio::test]
    async fn compiles_markdown_through_pandoc_and_latexmk() {
        if resolve_executable("latexmk").is_none() || resolve_executable("pandoc").is_none() {
            eprintln!("skipping: latexmk or pandoc is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("source");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("essay.md"),
            "---\ntitle: An Essay\nauthor: A. Leitao\n---\n\n\
             # Introduction\n\nMarkdown reaches latexmk through pandoc.\n",
        )
        .unwrap();

        let project = fixture_project(&root.join("essay.md"));
        let source = crate::sources::prepare(
            &project,
            &SourceRef::Worktree,
            &crate::database::Repository::open(&directory.path().join("press.db")).unwrap(),
            directory.path(),
        )
        .unwrap();
        let (_handle, cancel) = CancelHandle::new();

        let work = directory.path().join("work");
        let outcome = run(
            BuildInputs {
                build_id: 3,
                project: &project,
                source: &source,
                work_directory: work.clone(),
                log_path: work.join("last-build.log"),
                artifact_directory: directory.path().join("artifacts"),
            },
            cancel,
            Arc::new(PidRegistry::default()),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();

        let BuildOutcome::Succeeded { product, .. } = outcome else {
            panic!("a valid markdown document should compile");
        };
        assert!(product.pdf_path.is_file());
        assert_eq!(product.page_count, Some(1));
        verify_pdf(&product.pdf_path).unwrap();

        // pandoc's output is kept beside the auxiliary files, never in the
        // project folder.
        let generated = work.join("essay.tex");
        assert!(generated.is_file());
        assert!(!root.join("essay.tex").exists());
        assert!(
            std::fs::read_to_string(&generated).unwrap().contains("An Essay"),
            "the frontmatter reached the LaTeX"
        );

        // A rebuild with no edit must not rewrite the generated file, or
        // latexmk would redo work it had already cached.
        let stamp = std::fs::metadata(&generated).unwrap().modified().unwrap();
        let (_handle, cancel) = CancelHandle::new();
        run(
            BuildInputs {
                build_id: 4,
                project: &project,
                source: &source,
                work_directory: work.clone(),
                log_path: work.join("last-build.log"),
                artifact_directory: directory.path().join("artifacts"),
            },
            cancel,
            Arc::new(PidRegistry::default()),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::metadata(&generated).unwrap().modified().unwrap(),
            stamp,
            "an unchanged document leaves pandoc's output alone"
        );
    }

    #[test]
    fn markdown_diagnostics_point_at_the_markdown_not_the_generated_latex() {
        let generated = Path::new("/cache/work/essay.tex");
        let mut diagnostics = vec![
            Diagnostic {
                file: Some("/cache/work/essay.tex".into()),
                line: Some(214),
                severity: Severity::Error,
                message: "Undefined control sequence.".into(),
            },
            Diagnostic {
                file: Some("essay.tex".into()),
                line: Some(9),
                severity: Severity::Warning,
                message: "Overfull hbox".into(),
            },
            Diagnostic {
                file: Some("/usr/local/texlive/article.cls".into()),
                line: Some(5),
                severity: Severity::Error,
                message: "Something in a class file.".into(),
            },
        ];
        attribute_to_source(&mut diagnostics, generated, "essay.md");

        // Both spellings of the generated file move to the source, and lose the
        // line number, which referred to pandoc's output.
        assert_eq!(diagnostics[0].file.as_deref(), Some("essay.md"));
        assert_eq!(diagnostics[0].line, None);
        assert_eq!(diagnostics[1].file.as_deref(), Some("essay.md"));
        assert_eq!(diagnostics[1].line, None);
        // A real file somewhere else is left exactly as it was.
        assert_eq!(
            diagnostics[2].file.as_deref(),
            Some("/usr/local/texlive/article.cls")
        );
        assert_eq!(diagnostics[2].line, Some(5));
    }

    #[tokio::test]
    async fn a_broken_document_fails_with_a_located_diagnostic() {
        if resolve_executable("latexmk").is_none() {
            eprintln!("skipping: latexmk is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("source");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("main.tex"),
            "\\documentclass{article}\n\\begin{document}\nfine\n\\undefinedmacro\n\\end{document}\n",
        )
        .unwrap();

        let project = fixture_project(&root.join("main.tex"));
        let store = tempfile::tempdir().unwrap();
        let repository = crate::database::Repository::open(&store.path().join("press.db")).unwrap();
        let source = crate::sources::prepare(
            &project,
            &SourceRef::Worktree,
            &repository,
            store.path(),
        )
        .unwrap();
        let (_handle, cancel) = CancelHandle::new();
        let outcome = run(
            BuildInputs {
                build_id: 2,
                project: &project,
                source: &source,
                work_directory: directory.path().join("work"),
                log_path: directory.path().join("work/last-build.log"),
                artifact_directory: directory.path().join("artifacts"),
            },
            cancel,
            Arc::new(PidRegistry::default()),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();

        let BuildOutcome::Failed {
            diagnostics,
            summary,
        } = outcome
        else {
            panic!("an undefined control sequence should fail the build");
        };
        assert!(summary.contains("Undefined control sequence"), "{summary}");
        let located = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.file.as_deref() == Some("main.tex"))
            .expect("the error is attributed to the source file");
        assert_eq!(located.line, Some(4));
    }
}
