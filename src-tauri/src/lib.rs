mod build;
mod commands;
mod database;
mod diagnostics;
mod discovery;
mod editor;
mod error;
mod files;
mod intake;
mod model;
mod protocol;
mod render;
mod runner;
mod snapshot;
mod sources;
mod toolchain;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
                if let Some(path) =
                    path_argument(&arguments, Path::new(&working_directory))
                {
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
            commands::inspect_project,
            commands::inspect_document,
            commands::add_project,
            commands::open_project,
            commands::close_project,
            commands::build_project,
            commands::rename_project,
            commands::update_project_settings,
            commands::delete_project,
            commands::page_layout,
            commands::page_words,
            commands::search_document,
            commands::create_snapshot,
            commands::list_versions,
            commands::rename_snapshot,
            commands::delete_snapshot,
            commands::get_build_log,
            commands::launch_neovim,
            commands::editor_status,
            commands::take_pending_open,
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
    tauri::async_runtime::spawn(async move {
        let handle = app.clone();
        let resolved = tauri::async_runtime::spawn_blocking(move || {
            let state = handle.state::<AppState>();
            intake::resolve(&path, &state.repository)
        })
        .await;

        let request = match resolved {
            Ok(Ok(request)) => request,
            Ok(Err(error)) => model::OpenRequest {
                path: String::new(),
                project_id: None,
                report: None,
                message: Some(error.to_string()),
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
    let retained = retained.into_iter().collect::<HashSet<_>>();
    for entry in walkdir::WalkDir::new(artifact_root)
        .min_depth(1)
        .max_depth(5)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        let extension = path.extension().and_then(|value| value.to_str());
        let disposable = match extension {
            Some("next") => true,
            Some("pdf") => !retained.contains(path),
            _ => false,
        };
        if disposable {
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
        model::{DocumentKind, Engine, SourceRef},
    };

    #[test]
    fn reads_a_path_from_the_command_line() {
        let cwd = Path::new("/home/antonio/papers");
        // The editor passes an absolute path.
        assert_eq!(
            path_argument(
                &["press".into(), "/tmp/thesis/main.tex".into()],
                cwd
            ),
            Some(PathBuf::from("/tmp/thesis/main.tex"))
        );
        // A relative one belongs to the caller's directory, not to Press's.
        assert_eq!(
            path_argument(&["press".into(), "thesis/main.tex".into()], cwd),
            Some(PathBuf::from("/home/antonio/papers/thesis/main.tex"))
        );
        // Flags the platform adds are not paths.
        assert_eq!(
            path_argument(
                &["press".into(), "--flag".into(), "main.tex".into()],
                cwd
            ),
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

        let repository = Repository::open(&directory.path().join("press.db")).unwrap();
        let project = repository
            .upsert_project(NewProject {
                name: "Source",
                root_path: source.to_str().unwrap(),
                main_file: "main.tex",
                working_directory: ".",
                kind: DocumentKind::Latex,
                engine: Engine::PdfLatex,
            })
            .unwrap();

        let live = artifact_root.join(project.id.to_string()).join("worktree");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(work_root.join(project.id.to_string())).unwrap();
        let retained = live.join("retained.pdf");
        let orphan = live.join("orphan.pdf");
        let staging = live.join("interrupted.next");
        std::fs::write(&retained, b"%PDF-1.7").unwrap();
        std::fs::write(&orphan, b"%PDF-1.7").unwrap();
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
        std::fs::write(source.join("main.tex"), "\\documentclass{article}").unwrap();
        let capture = crate::snapshot::capture(&source, &objects_root, crate::snapshot::Scope::Folder).unwrap();
        repository
            .create_snapshot(project.id, &capture, "First", None)
            .unwrap();
        let orphan_object = objects_root.join("zz/orphaned");
        std::fs::create_dir_all(orphan_object.parent().unwrap()).unwrap();
        std::fs::write(&orphan_object, b"nothing refers to this").unwrap();

        sweep_storage(&artifact_root, &work_root, &objects_root, &repository);

        assert!(retained.is_file());
        assert!(!orphan.exists());
        assert!(!staging.exists());
        assert!(!ghost_artifacts.exists());
        assert!(!ghost_work.exists());
        assert!(work_root.join(project.id.to_string()).is_dir());
        assert!(!orphan_object.exists(), "unreferenced file contents are swept");
        for file in &capture.files {
            assert!(
                crate::snapshot::object_path(&objects_root, &file.object).is_file(),
                "a snapshot's contents survive the sweep"
            );
        }
    }
}
