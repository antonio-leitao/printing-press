mod build;
mod commands;
mod database;
mod discovery;
mod editor;
mod error;
mod model;
mod toolchain;

use std::{path::PathBuf, sync::Arc};

use build::BuildManager;
use database::Repository;
use tauri::Manager;

pub struct AppState {
    repository: Arc<Repository>,
    builds: Arc<BuildManager>,
    artifact_root: PathBuf,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            },
        ))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_root = app.path().app_data_dir()?;
            let cache_root = app.path().app_cache_dir()?;
            std::fs::create_dir_all(&data_root)?;
            std::fs::create_dir_all(&cache_root)?;
            let artifact_root = data_root.join("projects");
            let work_root = cache_root.join("projects");
            std::fs::create_dir_all(&artifact_root)?;
            std::fs::create_dir_all(&work_root)?;
            let repository = Arc::new(Repository::open(&data_root.join("press.sqlite3"))?);
            cleanup_orphaned_artifacts(&artifact_root, &repository);
            let builds = Arc::new(BuildManager::new(
                Arc::clone(&repository),
                artifact_root.clone(),
                work_root,
            ));
            app.manage(AppState {
                repository,
                builds,
                artifact_root,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::inspect_project,
            commands::add_project,
            commands::activate_project,
            commands::deactivate_project,
            commands::rebuild_project,
            commands::read_project_pdf,
            commands::get_build_log,
            commands::launch_neovim,
            commands::editor_status,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Press");

    app.run(|app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        ) {
            app_handle.state::<AppState>().builds.shutdown_now();
        }
    });
}

fn cleanup_orphaned_artifacts(root: &std::path::Path, repository: &Repository) {
    let Ok(retained) = repository.managed_pdf_paths() else {
        return;
    };
    let retained = retained
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    for entry in walkdir::WalkDir::new(root)
        .min_depth(1)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        let disposable = path
            .extension()
            .is_some_and(|extension| extension == "next")
            || (path.extension().is_some_and(|extension| extension == "pdf")
                && !retained.contains(path));
        if disposable {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::NewProject;

    #[test]
    fn startup_cleanup_retains_only_the_database_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let artifact_root = directory.path().join("artifacts");
        let artifact_directory = artifact_root.join("1/artifacts");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir_all(&artifact_directory).unwrap();
        let retained = artifact_directory.join("retained.pdf");
        let orphan = artifact_directory.join("orphan.pdf");
        let staging = artifact_directory.join("interrupted.next");
        std::fs::write(&retained, b"retained").unwrap();
        std::fs::write(&orphan, b"orphan").unwrap();
        std::fs::write(&staging, b"staging").unwrap();

        let repository = Repository::open(&directory.path().join("press.db")).unwrap();
        let project = repository
            .add_or_update_project(NewProject {
                name: "Source",
                root_path: source.to_str().unwrap(),
                main_file: "main.tex",
                working_directory: ".",
                engine: "pdflatex",
            })
            .unwrap();
        repository
            .record_build_success(project.id, 10, &retained)
            .unwrap();

        cleanup_orphaned_artifacts(&artifact_root, &repository);

        assert!(retained.is_file());
        assert!(!orphan.exists());
        assert!(!staging.exists());
    }
}
