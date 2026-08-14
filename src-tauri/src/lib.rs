mod anchors;
mod build;
mod commands;
mod database;
mod diagnostics;
mod documents;
mod editor;
mod error;
mod files;
mod model;
mod peek;
mod protocol;
mod render;
mod runner;
mod snapshot;
mod sources;
mod toolchain;
mod viewing;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use build::BuildManager;
use database::Repository;
use render::RenderPool;
use tauri::Manager;

pub struct AppState {
    repository: Arc<Repository>,
    builds: Arc<BuildManager>,
    renderer: Arc<RenderPool>,
    artifact_root: PathBuf,
    /// Snapshot file contents, addressed by hash. Data, not cache: losing this
    /// loses the history itself.
    objects_root: PathBuf,
    /// Something worth telling the user about how Press started, collected by
    /// the interface once it is listening.
    startup_notice: Mutex<Option<String>>,
    /// A path handed to Press from outside, waiting for the interface to come
    /// and collect it. Held rather than only emitted, because a launch from the
    /// editor arrives before the webview is listening for anything.
    pending_open: Mutex<Option<model::OpenRequest>>,
    /// PDFs Press is showing but does not own. Kept for as long as one is on
    /// screen and no longer; nothing about them is stored.
    viewing: Arc<viewing::Viewing>,
    /// How many paths are on their way to becoming open requests.
    ///
    /// Resolving one reads the folder it names — every LaTeX file in it, to tell
    /// the documents from the chapters — which for a large tree takes long
    /// enough that the interface can finish starting first, find nothing
    /// waiting, and put the library up a moment before a document arrives.
    /// This is how it knows to wait instead.
    expecting_open: AtomicUsize,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // A second launch does not start a second Press: its arguments are
        // handed to the instance already running. That is the whole mechanism
        // behind `:Press` in the editor.
        .plugin(tauri_plugin_single_instance::init(
            |app, arguments, working_directory| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                if let Some(path) = path_argument(&arguments, Path::new(&working_directory)) {
                    accept_open_request(app.clone(), path);
                }
            },
        ))
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol(protocol::SCHEME, protocol::handle)
        .setup(|app| {
            let data_root = app.path().app_data_dir()?;
            let cache_root = app.path().app_cache_dir()?;
            std::fs::create_dir_all(&data_root)?;
            std::fs::create_dir_all(&cache_root)?;
            // Published PDFs are data: losing them costs a rebuild of every
            // version. Auxiliary files are cache: losing them costs one slow build.
            let artifact_root = data_root.join("projects");
            let work_root = cache_root.join("projects");
            let objects_root = data_root.join("objects");
            std::fs::create_dir_all(&artifact_root)?;
            std::fs::create_dir_all(&work_root)?;
            std::fs::create_dir_all(&objects_root)?;

            let repository = Arc::new(Repository::open(&data_root.join("press.sqlite3"))?);
            // Anything the database wants to say about how it opened. The
            // interface collects it the same way it collects a path from the
            // command line.
            let startup_notice = repository.notice().map(ToOwned::to_owned);
            // The editor command is written once, on the first run, rather than
            // defaulted on every read. A machine with a terminal editor on it
            // gets one that opens it; a machine without gets the system's own
            // answer. Either way it is an ordinary setting from then on, and
            // Press does not offer an opinion about it again.
            if repository.setting(editor::SETTING)?.is_none() {
                repository.set_setting(editor::SETTING, &editor::suggested_command())?;
            }
            sweep_storage(&artifact_root, &work_root, &objects_root, &repository);
            let builds = Arc::new(BuildManager::new(
                Arc::clone(&repository),
                artifact_root.clone(),
                work_root,
                objects_root.clone(),
            ));
            app.manage(AppState {
                repository,
                builds,
                renderer: Arc::new(RenderPool::with_default_size()),
                artifact_root,
                objects_root,
                pending_open: Mutex::new(None),
                startup_notice: Mutex::new(startup_notice),
                viewing: Arc::new(viewing::Viewing::default()),
                expecting_open: AtomicUsize::new(0),
            });

            // Press may have been started by the editor rather than by hand.
            let arguments = std::env::args().collect::<Vec<_>>();
            let working_directory = std::env::current_dir().unwrap_or_default();
            if let Some(path) = path_argument(&arguments, &working_directory) {
                accept_open_request(app.handle().clone(), path);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::resolve_path,
            commands::add_project,
            commands::open_project,
            commands::open_pdf,
            commands::close_pdf,
            commands::close_project,
            commands::build_project,
            commands::rename_project,
            commands::set_project_engine,
            commands::set_project_pinned,
            commands::delete_project,
            commands::page_layout,
            commands::page_words,
            commands::page_links,
            commands::open_external,
            commands::peek_source,
            commands::search_document,
            commands::create_snapshot,
            commands::list_versions,
            commands::rename_snapshot,
            commands::delete_snapshot,
            commands::export_artifact,
            commands::get_build_log,
            commands::launch_editor,
            commands::editor_command,
            commands::set_editor_command,
            commands::take_pending_open,
            commands::expecting_open,
            commands::take_startup_notice,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Press");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app_handle.state::<AppState>().builds.shutdown_now();
        }
    });
}

/// Counts one path down again however its resolution ends, including a panic.
struct Resolving(tauri::AppHandle);

impl Drop for Resolving {
    fn drop(&mut self) {
        self.0
            .state::<AppState>()
            .expecting_open
            .fetch_sub(1, Ordering::SeqCst);
    }
}

/// The first argument that looks like a path rather than a flag. Relative paths
/// are resolved against the caller's directory, not Press's.
fn path_argument(arguments: &[String], working_directory: &Path) -> Option<PathBuf> {
    arguments
        .iter()
        .skip(1)
        .find(|argument| !argument.starts_with('-') && !argument.is_empty())
        .map(|argument| {
            let path = PathBuf::from(argument);
            if path.is_absolute() {
                path
            } else {
                working_directory.join(path)
            }
        })
}

/// Resolves a path off the main thread, stores the result, and nudges the
/// interface. Storing it is what makes this work at startup, when nothing is
/// listening yet.
fn accept_open_request(app: tauri::AppHandle, path: PathBuf) {
    // Counted before the work starts, not after: the whole point is to be true
    // during the resolving.
    app.state::<AppState>()
        .expecting_open
        .fetch_add(1, Ordering::SeqCst);
    tauri::async_runtime::spawn(async move {
        // Whatever happens below, the interface stops waiting on this one.
        let _resolving = Resolving(app.clone());
        let handle = app.clone();
        let resolved = tauri::async_runtime::spawn_blocking(move || {
            let state = handle.state::<AppState>();
            documents::resolve(&path, &state.repository)
        })
        .await;

        let request = match resolved {
            Ok(Ok(request)) => request,
            Ok(Err(error)) => model::OpenRequest {
                path: String::new(),
                candidates: Vec::new(),
                pdf: None,
                warnings: vec![error.to_string()],
                toolchain: documents::toolchain(),
            },
            Err(error) => {
                eprintln!("Press could not resolve a path from the command line: {error}");
                return;
            }
        };

        if let Ok(mut pending) = app.state::<AppState>().pending_open.lock() {
            *pending = Some(request);
        }
        // The interface collects it; this only tells it there is something to
        // collect, so a missed event costs nothing.
        let _ = tauri::Emitter::emit(&app, "open-requested", ());
    });
}

/// Removes what nothing references any more: interrupted staging files, PDFs no
/// longer in the database, the storage of deleted projects, and file contents
/// belonging to discarded versions.
///
/// Runs at startup, when nothing else is writing to any of it.
fn sweep_storage(
    artifact_root: &Path,
    work_root: &Path,
    objects_root: &Path,
    repository: &Repository,
) {
    let Ok(retained) = repository.managed_pdf_paths() else {
        return;
    };
    // Kept by publication rather than by file. A build publishes a PDF and the
    // SyncTeX data beside it under one `build-<stamp>` name, and they are only
    // worth anything together: matching on the extension alone kept every
    // sidecar a superseded build ever wrote, once per save, for good.
    let retained = retained
        .iter()
        .filter_map(|pdf| Some(pdf.parent()?.join(runner::publication_stem(pdf)?)))
        .collect::<HashSet<_>>();
    for entry in walkdir::WalkDir::new(artifact_root)
        .min_depth(1)
        .max_depth(5)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        // Everything under here is something Press published, so anything not
        // belonging to a publication the database still names is rubbish —
        // interrupted staging files among them.
        let publication = path
            .parent()
            .zip(runner::publication_stem(path))
            .map(|(directory, stem)| directory.join(stem));
        if !publication.is_some_and(|publication| retained.contains(&publication)) {
            let _ = std::fs::remove_file(path);
        }
    }

    sweep_objects(objects_root, repository);

    let Ok(known) = repository.project_ids() else {
        return;
    };
    let known = known
        .into_iter()
        .map(|id| id.to_string())
        .collect::<HashSet<_>>();
    for root in [artifact_root, work_root] {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Directories are named after project ids; anything else is a leftover.
            if entry.path().is_dir() && !known.contains(&name) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// Snapshots share file contents, so an object only becomes rubbish once no
/// snapshot at all refers to it.
fn sweep_objects(objects_root: &Path, repository: &Repository) {
    let Ok(referenced) = repository.referenced_objects() else {
        return;
    };
    let referenced = referenced.into_iter().collect::<HashSet<_>>();
    for entry in walkdir::WalkDir::new(objects_root)
        .min_depth(2)
        .max_depth(2)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        // The name is the hash with its two-character shard split off.
        let Some(hash) = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|shard| shard.to_str())
            .zip(path.file_name().and_then(|name| name.to_str()))
            .map(|(shard, rest)| format!("{shard}{rest}"))
        else {
            continue;
        };
        if !referenced.contains(&hash) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database::{NewArtifact, NewProject},
        model::{Engine, SourceRef},
    };

    #[test]
    fn reads_a_path_from_the_command_line() {
        let cwd = Path::new("/home/antonio/papers");
        // The editor passes an absolute path.
        assert_eq!(
            path_argument(&["press".into(), "/tmp/thesis/main.tex".into()], cwd),
            Some(PathBuf::from("/tmp/thesis/main.tex"))
        );
        // A relative one belongs to the caller's directory, not to Press's.
        assert_eq!(
            path_argument(&["press".into(), "thesis/main.tex".into()], cwd),
            Some(PathBuf::from("/home/antonio/papers/thesis/main.tex"))
        );
        // Flags the platform adds are not paths.
        assert_eq!(
            path_argument(&["press".into(), "--flag".into(), "main.tex".into()], cwd),
            Some(PathBuf::from("/home/antonio/papers/main.tex"))
        );
        assert_eq!(path_argument(&["press".into()], cwd), None);
        assert_eq!(path_argument(&[], cwd), None);
    }

    #[test]
    fn startup_keeps_only_what_the_database_still_references() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let artifact_root = directory.path().join("artifacts");
        let work_root = directory.path().join("work");
        let objects_root = directory.path().join("objects");
        std::fs::create_dir(&source).unwrap();

        std::fs::write(source.join("main.tex"), "\\documentclass{article}").unwrap();
        let repository = Repository::open(&directory.path().join("press.db")).unwrap();
        let project = repository
            .upsert_project(NewProject {
                name: "source/main.tex",
                document_path: source.join("main.tex").to_str().unwrap(),
                engine: Engine::PdfLatex,
            })
            .unwrap();

        let live = artifact_root.join(project.id.to_string()).join("worktree");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(work_root.join(project.id.to_string())).unwrap();
        // A publication is a PDF and the sync data published beside it, under
        // one name. Both of these are whole publications; only one is still
        // referenced.
        let retained = live.join("build-2.pdf");
        let retained_sync = live.join("build-2.synctex.gz");
        let retained_lines = live.join("build-2.lines");
        let orphan = live.join("build-1.pdf");
        let orphan_sync = live.join("build-1.synctex.gz");
        let staging = live.join("build-3.next");
        std::fs::write(&retained, b"%PDF-1.7").unwrap();
        std::fs::write(&retained_sync, b"sync").unwrap();
        std::fs::write(&retained_lines, b"1:1").unwrap();
        std::fs::write(&orphan, b"%PDF-1.7").unwrap();
        std::fs::write(&orphan_sync, b"sync").unwrap();
        std::fs::write(&staging, b"partial").unwrap();
        repository
            .record_artifact(NewArtifact {
                project_id: project.id,
                source_ref: &SourceRef::Worktree,
                engine: Engine::PdfLatex,
                pdf_path: &retained,
                page_count: Some(1),
                byte_size: 8,
            })
            .unwrap();

        // Storage belonging to a project that no longer exists.
        let ghost_artifacts = artifact_root.join("9999");
        let ghost_work = work_root.join("9999");
        std::fs::create_dir_all(ghost_artifacts.join("worktree")).unwrap();
        std::fs::create_dir_all(&ghost_work).unwrap();

        // One object is still referenced by a snapshot; the other is not.
        let capture = crate::snapshot::capture(&source, &objects_root, &HashSet::new()).unwrap();
        repository
            .create_snapshot(project.id, &capture, "First", None)
            .unwrap()
            .stored()
            .expect("nothing like it is stored yet");
        let orphan_object = objects_root.join("zz/orphaned");
        std::fs::create_dir_all(orphan_object.parent().unwrap()).unwrap();
        std::fs::write(&orphan_object, b"nothing refers to this").unwrap();

        sweep_storage(&artifact_root, &work_root, &objects_root, &repository);

        assert!(retained.is_file());
        assert!(
            retained_sync.is_file() && retained_lines.is_file(),
            "a live PDF keeps the sync data that lets it be clicked through"
        );
        assert!(!orphan.exists());
        assert!(
            !orphan_sync.exists(),
            "a superseded build's sidecars go with it, or every save leaks one"
        );
        assert!(!staging.exists());
        assert!(!ghost_artifacts.exists());
        assert!(!ghost_work.exists());
        assert!(work_root.join(project.id.to_string()).is_dir());
        assert!(
            !orphan_object.exists(),
            "unreferenced file contents are swept"
        );
        for file in &capture.files {
            assert!(
                crate::snapshot::object_path(&objects_root, &file.object).is_file(),
                "a snapshot's contents survive the sweep"
            );
        }
    }
}
