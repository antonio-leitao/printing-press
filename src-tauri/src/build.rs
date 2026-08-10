//! Scheduling builds.
//!
//! One project's working tree is watched at a time, but any number of versions
//! can be building at once: the queue is keyed on `(project, source_ref)`, which
//! is the shape the history sidebar needs when it opens three past versions at
//! once and expects the interface to stay live.
//!
//! Coalescing rather than cancelling is deliberate. A save during a build marks
//! that build dirty; the build still finishes and still publishes, and only then
//! runs again. Discarding a finished PDF because a keystroke landed means a
//! document that saves faster than it compiles never updates at all.
//!
//! Every write to the database, and every restore of a version, goes through
//! `spawn_blocking`: a write commits to the disk and a restore copies a whole
//! source tree, and neither belongs on a thread that is meant to be driving
//! builds and serving pages. The handful of single-row *reads* left inline are
//! deliberate — one indexed lookup against a warm SQLite page cache, where the
//! hop off the runtime would cost about what the query does.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Semaphore, mpsc};

use crate::{
    database::{NewArtifact, Repository},
    diagnostics::ProgressSnapshot,
    error::{AppError, AppResult},
    files,
    model::{
        ArtifactSummary, BuildProgress, BuildState, BuildStatus, BuildUpdate, Diagnostic, Project,
        SourceRef,
    },
    runner::{self, BuildInputs, BuildOutcome, Cancel, CancelHandle, PidRegistry, ProgressSink},
    sources,
};

/// LaTeX builds are single-threaded and heavy; a few at once keeps the machine
/// responsive while still letting the history open several versions.
const MAX_CONCURRENT_BUILDS: usize = 3;
const DEBOUNCE: Duration = Duration::from_millis(250);

type BuildKey = (i64, SourceRef);

pub struct BuildManager {
    repository: Arc<Repository>,
    artifact_root: PathBuf,
    work_root: PathBuf,
    /// Where snapshot file contents live, for restoring a version to build it.
    objects_root: PathBuf,
    pids: Arc<PidRegistry>,
    permits: Arc<Semaphore>,
    state: Mutex<ManagerState>,
}

#[derive(Default)]
struct ManagerState {
    next_build_id: u64,
    active: HashMap<BuildKey, ActiveBuild>,
    watch: Option<WatchHandle>,
}

struct ActiveBuild {
    build_id: u64,
    cancel: CancelHandle,
    /// The source changed while this build was running, so run again once it has
    /// published.
    dirty: bool,
}

struct WatchHandle {
    project_id: i64,
    cancel: CancelHandle,
}

enum WatchMessage {
    Changed,
    Failed(String),
}

impl BuildManager {
    pub fn new(
        repository: Arc<Repository>,
        artifact_root: PathBuf,
        work_root: PathBuf,
        objects_root: PathBuf,
    ) -> Self {
        Self {
            repository,
            artifact_root,
            work_root,
            objects_root,
            pids: Arc::new(PidRegistry::default()),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_BUILDS)),
            state: Mutex::new(ManagerState::default()),
        }
    }

    /// Makes a project current: stops watching whatever was open, watches this
    /// one, and starts a build of its working tree.
    pub async fn open(self: Arc<Self>, app: AppHandle, project: Project) -> AppResult<()> {
        self.close_others(project.id).await;
        // A write, so off the runtime like the others.
        let repository = Arc::clone(&self.repository);
        let project_id = project.id;
        tauri::async_runtime::spawn_blocking(move || repository.touch_project(project_id))
            .await
            .map_err(|error| AppError::Task(error.to_string()))??;
        Arc::clone(&self).start_watching(&app, &project).await;
        self.request(app, project, SourceRef::Worktree).await?;
        Ok(())
    }

    /// Stops watching and cancels everything in flight.
    pub async fn close(&self) {
        let (watch, builds) = {
            let mut state = self.state.lock().await;
            (
                state.watch.take(),
                state
                    .active
                    .drain()
                    .map(|(_, build)| build)
                    .collect::<Vec<_>>(),
            )
        };
        if let Some(watch) = watch {
            watch.cancel.cancel();
        }
        for build in builds {
            build.cancel.cancel();
        }
    }

    async fn close_others(&self, keep: i64) {
        let (watch, builds) = {
            let mut state = self.state.lock().await;
            let watch = match &state.watch {
                Some(handle) if handle.project_id != keep => state.watch.take(),
                _ => None,
            };
            let stale = state
                .active
                .keys()
                .filter(|(project_id, _)| *project_id != keep)
                .cloned()
                .collect::<Vec<_>>();
            let builds = stale
                .into_iter()
                .filter_map(|key| state.active.remove(&key))
                .collect::<Vec<_>>();
            (watch, builds)
        };
        if let Some(watch) = watch {
            watch.cancel.cancel();
        }
        for build in builds {
            build.cancel.cancel();
        }
    }

    /// Queues a build, or marks the one already running for this version dirty.
    /// Returns the build that will satisfy the request.
    pub async fn request(
        self: Arc<Self>,
        app: AppHandle,
        project: Project,
        source_ref: SourceRef,
    ) -> AppResult<u64> {
        let key = (project.id, source_ref.clone());
        let (build_id, cancel) = {
            let mut state = self.state.lock().await;
            if let Some(active) = state.active.get_mut(&key) {
                active.dirty = true;
                return Ok(active.build_id);
            }
            state.next_build_id += 1;
            let build_id = state.next_build_id;
            let (handle, cancel) = CancelHandle::new();
            state.active.insert(
                key,
                ActiveBuild {
                    build_id,
                    cancel: handle,
                    dirty: false,
                },
            );
            (build_id, cancel)
        };

        tauri::async_runtime::spawn(async move {
            self.drive(app, project, source_ref, build_id, cancel).await;
        });
        Ok(build_id)
    }

    /// Runs a build, then repeats while it keeps being marked dirty.
    async fn drive(
        self: Arc<Self>,
        app: AppHandle,
        project: Project,
        source_ref: SourceRef,
        build_id: u64,
        cancel: Cancel,
    ) {
        let key = (project.id, source_ref.clone());
        // Kept so a cancelled build can put back what was on screen. Without
        // this, closing a project mid-build leaves it recorded as running
        // forever, because a cancelled build records no result of its own.
        let settled = self
            .repository
            .build_state(project.id, &source_ref)
            .unwrap_or_else(|_| BuildState::never(source_ref.clone()));

        self.record(
            &app,
            build_id,
            project.id,
            BuildState {
                source_ref: source_ref.clone(),
                status: BuildStatus::Queued,
                started_at: Some(now()),
                finished_at: None,
                duration_ms: None,
                error_summary: None,
                diagnostics: Vec::new(),
            },
            None,
        )
        .await;

        loop {
            // Waiting for a permit is the queued state; the interface stays live.
            let Ok(permit) = Arc::clone(&self.permits).acquire_owned().await else {
                break;
            };
            if cancel.is_cancelled() {
                break;
            }
            self.run_once(&app, &project, &source_ref, build_id, cancel.clone())
                .await;
            drop(permit);
            if cancel.is_cancelled() {
                break;
            }

            let mut state = self.state.lock().await;
            match state.active.get_mut(&key) {
                // Something changed while we were building: publish happened,
                // now go again.
                Some(active) if active.build_id == build_id && active.dirty => {
                    active.dirty = false;
                }
                _ => break,
            }
        }

        let superseded = {
            let mut state = self.state.lock().await;
            match state.active.get(&key) {
                Some(active) if active.build_id == build_id => {
                    state.active.remove(&key);
                    false
                }
                // A newer build already owns this version.
                Some(_) => true,
                None => false,
            }
        };

        // A cancelled build records no result of its own, so the state it wrote
        // on the way in has to be undone. Skipped when a newer build has taken
        // over, which would otherwise be overwritten with stale state.
        if cancel.is_cancelled() && !superseded {
            self.record(&app, build_id, project.id, settled, None).await;
        }
    }

    async fn run_once(
        &self,
        app: &AppHandle,
        project: &Project,
        source_ref: &SourceRef,
        build_id: u64,
        cancel: Cancel,
    ) {
        let started = Instant::now();
        let started_at = now();
        self.record(
            app,
            build_id,
            project.id,
            BuildState {
                source_ref: source_ref.clone(),
                status: BuildStatus::Running,
                started_at: Some(started_at),
                finished_at: None,
                duration_ms: None,
                error_summary: None,
                diagnostics: Vec::new(),
            },
            None,
        )
        .await;

        // Off the runtime. Restoring a version copies every file it holds out of
        // the object store — up to the store's own half-gigabyte limit — and
        // doing that on a runtime thread stalls every other build, every command
        // the interface has asked for, and the page currently being fetched.
        let prepared = {
            let project = project.clone();
            let source_ref = source_ref.clone();
            let repository = Arc::clone(&self.repository);
            let objects = self.objects_root.clone();
            tauri::async_runtime::spawn_blocking(move || {
                sources::prepare(&project, &source_ref, &repository, &objects)
            })
            .await
        };
        let source = match prepared.map_err(|error| AppError::Task(error.to_string())) {
            Ok(Ok(source)) => source,
            Ok(Err(error)) | Err(error) => {
                self.finish_with_error(
                    app,
                    build_id,
                    project.id,
                    source_ref,
                    started_at,
                    started.elapsed(),
                    error.to_string(),
                    Vec::new(),
                )
                .await;
                return;
            }
        };

        // The previous build's page count is the denominator for the progress
        // banner: "page 30 of about 42".
        let expected_pages = self
            .repository
            .artifact_for(project.id, source_ref, project.engine)
            .ok()
            .flatten()
            .and_then(|stored| stored.summary.page_count);

        let sink = progress_sink(
            app.clone(),
            build_id,
            project.id,
            source_ref.clone(),
            expected_pages,
        );
        let inputs = BuildInputs {
            build_id,
            project,
            source: &source,
            work_directory: self.work_directory(project.id, source_ref),
            log_path: self.log_path(project.id, source_ref),
            artifact_directory: self.artifact_directory(project.id, source_ref),
        };

        let outcome = runner::run(inputs, cancel.clone(), Arc::clone(&self.pids), sink).await;
        let elapsed = started.elapsed();

        match outcome {
            Ok(BuildOutcome::Cancelled) => {
                // Nothing is recorded: a cancelled build is not a result, and the
                // last good PDF stays on screen.
            }
            Ok(BuildOutcome::Succeeded {
                product,
                diagnostics,
            }) => {
                // Publishing is a transaction, and a transaction commits to the
                // disk. Off the runtime for the same reason every other write
                // is: how long a sync takes is the filesystem's business, not
                // something a thread meant to be driving builds should wait on.
                let recorded = {
                    let repository = Arc::clone(&self.repository);
                    let project_id = project.id;
                    let source_ref = source_ref.clone();
                    let engine = project.engine;
                    let pdf_path = product.pdf_path.clone();
                    let page_count = product.page_count;
                    let byte_size = product.byte_size;
                    tauri::async_runtime::spawn_blocking(move || {
                        repository.record_artifact(NewArtifact {
                            project_id,
                            source_ref: &source_ref,
                            engine,
                            pdf_path: &pdf_path,
                            page_count,
                            byte_size,
                        })
                    })
                    .await
                    .unwrap_or_else(|error| Err(AppError::Task(error.to_string())))
                };
                match recorded {
                    Ok((artifact, superseded)) => {
                        if let Some(previous) = superseded {
                            runner::discard_publication(&previous).await;
                        }
                        self.record(
                            app,
                            build_id,
                            project.id,
                            BuildState {
                                source_ref: source_ref.clone(),
                                status: BuildStatus::Success,
                                started_at: Some(started_at),
                                finished_at: Some(now()),
                                duration_ms: Some(elapsed.as_millis() as i64),
                                error_summary: None,
                                diagnostics,
                            },
                            Some(artifact),
                        )
                        .await;
                    }
                    Err(error) => {
                        // The PDF exists but could not be recorded, so it would
                        // never be found again.
                        runner::discard_publication(&product.pdf_path).await;
                        self.finish_with_error(
                            app,
                            build_id,
                            project.id,
                            source_ref,
                            started_at,
                            elapsed,
                            format!("could not record the built PDF: {error}"),
                            Vec::new(),
                        )
                        .await;
                    }
                }
            }
            Ok(BuildOutcome::Failed {
                diagnostics,
                summary,
            }) => {
                self.finish_with_error(
                    app,
                    build_id,
                    project.id,
                    source_ref,
                    started_at,
                    elapsed,
                    summary,
                    diagnostics,
                )
                .await;
            }
            Err(error) => {
                self.finish_with_error(
                    app,
                    build_id,
                    project.id,
                    source_ref,
                    started_at,
                    elapsed,
                    error.to_string(),
                    Vec::new(),
                )
                .await;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_with_error(
        &self,
        app: &AppHandle,
        build_id: u64,
        project_id: i64,
        source_ref: &SourceRef,
        started_at: i64,
        elapsed: Duration,
        summary: String,
        diagnostics: Vec<Diagnostic>,
    ) {
        self.record(
            app,
            build_id,
            project_id,
            BuildState {
                source_ref: source_ref.clone(),
                status: BuildStatus::Error,
                started_at: Some(started_at),
                finished_at: Some(now()),
                duration_ms: Some(elapsed.as_millis() as i64),
                error_summary: Some(summary),
                diagnostics,
            },
            None,
        )
        .await;
    }

    /// Persists a build state and tells the interface about it. The artifact is
    /// looked up when not supplied so every update carries the PDF that is
    /// currently on screen.
    async fn record(
        &self,
        app: &AppHandle,
        build_id: u64,
        project_id: i64,
        state: BuildState,
        artifact: Option<ArtifactSummary>,
    ) {
        // The four queries below go together and go off the runtime together —
        // one hop rather than four, and none of them on a thread that is meant
        // to be driving builds and serving pages. This runs on every state a
        // build passes through.
        let repository = Arc::clone(&self.repository);
        let stored = state.clone();
        let written = tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = repository.set_build_state(project_id, &stored) {
                eprintln!("Press could not record build state: {error}");
            }
            let artifact = artifact.or_else(|| {
                let engine = repository.get_project(project_id).ok()?.engine;
                repository
                    .artifact_for(project_id, &stored.source_ref, engine)
                    .ok()
                    .flatten()
                    .map(|stored| stored.summary)
            });
            // The library grid shows per-project state, so keep it in step.
            (artifact, repository.project_summary(project_id).ok())
        })
        .await;
        let (artifact, summary) = written.unwrap_or_default();

        let _ = app.emit(
            "build-updated",
            BuildUpdate {
                build_id: Some(build_id),
                project_id,
                source_ref: state.source_ref.clone(),
                build: state,
                artifact,
            },
        );
        if let Some(summary) = summary {
            let _ = app.emit("project-updated", summary);
        }
    }

    // -- watching ---------------------------------------------------------

    async fn start_watching(self: Arc<Self>, app: &AppHandle, project: &Project) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let watcher_sender = sender.clone();
        let directory = project.directory();
        // Read once, when the project is opened: adding a sibling project while
        // this one is on screen is not worth a channel for.
        let foreign = self
            .repository
            .foreign_documents(project.id, &directory)
            .unwrap_or_default();
        let scope = WatchScope {
            directory: directory.clone(),
            foreign: foreign.into_iter().collect(),
        };
        let watcher =
            notify::recommended_watcher(move |result: notify::Result<Event>| match result {
                Ok(event) if scope.relevant(&event) => {
                    let _ = watcher_sender.send(WatchMessage::Changed);
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = watcher_sender.send(WatchMessage::Failed(error.to_string()));
                }
            });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                // A project without a watcher still builds on request. This is a
                // degraded mode, not a failed document, so it never touches
                // build state.
                emit_watcher_error(app, project.id, &error.to_string());
                return;
            }
        };
        if let Err(error) = watcher.watch(&directory, RecursiveMode::Recursive) {
            emit_watcher_error(app, project.id, &error.to_string());
            return;
        }

        let (handle, cancel) = CancelHandle::new();
        {
            let mut state = self.state.lock().await;
            if let Some(previous) = state.watch.replace(WatchHandle {
                project_id: project.id,
                cancel: handle,
            }) {
                previous.cancel.cancel();
            }
        }

        let app = app.clone();
        let project = project.clone();
        tauri::async_runtime::spawn(async move {
            watch_loop(self, app, project, receiver, cancel, watcher).await;
        });
    }

    /// Cancels the build of one version, leaving the project's others alone.
    pub async fn cancel_version(&self, project_id: i64, source_ref: &SourceRef) {
        let build = {
            let mut state = self.state.lock().await;
            state.active.remove(&(project_id, source_ref.clone()))
        };
        if let Some(build) = build {
            build.cancel.cancel();
        }
    }

    /// Cancels every build for one project, without touching the others.
    pub async fn cancel_project(&self, project_id: i64) {
        let (watch, builds) = {
            let mut state = self.state.lock().await;
            let watch = match &state.watch {
                Some(handle) if handle.project_id == project_id => state.watch.take(),
                _ => None,
            };
            let keys = state
                .active
                .keys()
                .filter(|(owner, _)| *owner == project_id)
                .cloned()
                .collect::<Vec<_>>();
            let builds = keys
                .into_iter()
                .filter_map(|key| state.active.remove(&key))
                .collect::<Vec<_>>();
            (watch, builds)
        };
        if let Some(watch) = watch {
            watch.cancel.cancel();
        }
        for build in builds {
            build.cancel.cancel();
        }
    }

    // -- paths ------------------------------------------------------------

    fn version_directory(&self, root: &Path, project_id: i64, source_ref: &SourceRef) -> PathBuf {
        root.join(project_id.to_string()).join(source_ref.slug())
    }

    fn work_directory(&self, project_id: i64, source_ref: &SourceRef) -> PathBuf {
        self.version_directory(&self.work_root, project_id, source_ref)
            .join("work")
    }

    pub fn log_path(&self, project_id: i64, source_ref: &SourceRef) -> PathBuf {
        self.version_directory(&self.work_root, project_id, source_ref)
            .join("last-build.log")
    }

    fn artifact_directory(&self, project_id: i64, source_ref: &SourceRef) -> PathBuf {
        self.version_directory(&self.artifact_root, project_id, source_ref)
    }

    /// Removes everything Press generated for a project. Called after the
    /// project itself has been deleted.
    pub async fn discard_project_storage(&self, project_id: i64) {
        for root in [&self.artifact_root, &self.work_root] {
            let _ = tokio::fs::remove_dir_all(root.join(project_id.to_string())).await;
        }
    }

    /// Signals every live build without waiting. Only for application exit.
    pub fn shutdown_now(&self) {
        self.pids.terminate_all();
    }
}

async fn watch_loop(
    manager: Arc<BuildManager>,
    app: AppHandle,
    project: Project,
    mut events: mpsc::UnboundedReceiver<WatchMessage>,
    cancel: Cancel,
    _watcher: notify::RecommendedWatcher,
) {
    loop {
        let message = tokio::select! {
            () = cancel.cancelled() => return,
            message = events.recv() => message,
        };
        match message {
            None => return,
            Some(WatchMessage::Failed(error)) => {
                emit_watcher_error(&app, project.id, &error);
                continue;
            }
            Some(WatchMessage::Changed) => {}
        }

        // Coalesce a burst of saves into one build.
        loop {
            let deadline = tokio::time::sleep(DEBOUNCE);
            tokio::pin!(deadline);
            tokio::select! {
                () = &mut deadline => break,
                () = cancel.cancelled() => return,
                message = events.recv() => match message {
                    None => return,
                    Some(WatchMessage::Failed(error)) => {
                        emit_watcher_error(&app, project.id, &error);
                    }
                    Some(WatchMessage::Changed) => {}
                },
            }
        }

        // Re-read the project so an edit to its settings is picked up.
        let current = manager
            .repository
            .get_project(project.id)
            .unwrap_or_else(|_| project.clone());
        if let Err(error) = Arc::clone(&manager)
            .request(app.clone(), current, SourceRef::Worktree)
            .await
        {
            eprintln!("Press could not queue a build: {error}");
        }
    }
}

/// A broken watcher is a Press problem, not a document problem, and is reported
/// on its own channel so it never appears as a compile error.
fn emit_watcher_error(app: &AppHandle, project_id: i64, message: &str) {
    let _ = app.emit(
        "watcher-error",
        serde_json::json!({
            "projectId": project_id,
            "message": format!("Press cannot watch this folder for changes: {message}"),
        }),
    );
}

fn progress_sink(
    app: AppHandle,
    build_id: u64,
    project_id: i64,
    source_ref: SourceRef,
    expected_pages: Option<i64>,
) -> ProgressSink {
    Arc::new(move |snapshot: ProgressSnapshot| {
        let _ = app.emit(
            "build-progress",
            BuildProgress {
                build_id,
                project_id,
                source_ref: source_ref.clone(),
                stage: snapshot.stage,
                pass: snapshot.pass,
                page: snapshot.page,
                expected_pages,
            },
        );
    })
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// A change is worth rebuilding for when it touches something the author wrote.
/// Reads and build output are not that.
/// Which saves under a project's directory are this project's business.
struct WatchScope {
    directory: PathBuf,
    /// The documents of the other projects here, relative to `directory`.
    foreign: HashSet<String>,
}

impl WatchScope {
    fn relevant(&self, event: &Event) -> bool {
        if matches!(event.kind, EventKind::Access(_)) {
            return false;
        }
        event.paths.iter().any(|path| self.worth_rebuilding(path))
    }

    /// A save rebuilds this document unless the file saved is another project's
    /// document. Everything else in the directory is shared — a figure, a `.bib`,
    /// a chapter — and rebuilding for those is the point.
    fn worth_rebuilding(&self, path: &Path) -> bool {
        let relative = path.strip_prefix(&self.directory).unwrap_or(path);
        files::belongs_to_project(relative, &self.foreign)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn event(path: &str) -> Event {
        Event {
            kind: EventKind::Any,
            paths: vec![PathBuf::from(path)],
            attrs: Default::default(),
        }
    }

    fn scope(foreign: &[&str]) -> WatchScope {
        WatchScope {
            directory: PathBuf::from("/paper"),
            foreign: foreign.iter().map(|path| (*path).to_owned()).collect(),
        }
    }

    #[test]
    fn source_changes_trigger_builds_but_generated_files_do_not() {
        let scope = scope(&[]);
        assert!(scope.relevant(&event("/paper/chapter.tex")));
        assert!(scope.relevant(&event("/paper/figures/data.csv")));
        assert!(scope.relevant(&event("/paper/references.bib")));
        // PDF figures are ordinary source: regenerating a plot must rebuild.
        // Press writes its own output outside the project, so there is no loop.
        assert!(scope.relevant(&event("/paper/figures/plot.pdf")));
        assert!(!scope.relevant(&event("/paper/main.aux")));
        assert!(!scope.relevant(&event("/paper/main.log")));
        assert!(!scope.relevant(&event("/paper/.git/index")));
        assert!(!scope.relevant(&event("/paper/.main.tex.swp")));
        assert!(!scope.relevant(&event("/paper/4913")));
        assert!(!scope.relevant(&event("/paper/.#main.tex")));
    }

    /// One rule for every kind of document: a neighbour that is its own project
    /// does not rebuild this one, and the assets they share do.
    #[test]
    fn a_neighbouring_project_does_not_rebuild_this_one() {
        let scope = scope(&["talk.md", "supplementary.tex"]);

        assert!(
            scope.relevant(&event("/paper/essay.md")),
            "its own document"
        );
        // Other documents in the same folder, which are other projects.
        assert!(!scope.relevant(&event("/paper/talk.md")));
        assert!(!scope.relevant(&event("/paper/supplementary.tex")));
        // A chapter nothing else claims is still this document's business.
        assert!(scope.relevant(&event("/paper/chapters/one.tex")));
        // Assets are shared, so they still count.
        assert!(scope.relevant(&event("/paper/figures/plot.png")));
        assert!(scope.relevant(&event("/paper/references.bib")));
    }

    #[test]
    fn versions_get_separate_scratch_space() {
        let manager = BuildManager::new(
            Arc::new(
                Repository::open(
                    &std::env::temp_dir().join(format!("press-test-{}.db", std::process::id())),
                )
                .unwrap(),
            ),
            PathBuf::from("/artifacts"),
            PathBuf::from("/work"),
            PathBuf::from("/objects"),
        );
        let worktree = manager.work_directory(7, &SourceRef::Worktree);
        let snapshot = manager.work_directory(7, &SourceRef::Snapshot("abc".into()));
        assert_ne!(worktree, snapshot);
        assert!(worktree.starts_with("/work/7"));
        assert!(snapshot.starts_with("/work/7"));
        // Two versions building at once must not share an auxiliary directory.
        assert_ne!(
            manager.artifact_directory(7, &SourceRef::Worktree),
            manager.artifact_directory(7, &SourceRef::Snapshot("abc".into()))
        );
        let _ = std::fs::remove_file(
            std::env::temp_dir().join(format!("press-test-{}.db", std::process::id())),
        );
    }
}
