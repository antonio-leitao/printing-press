use std::{path::PathBuf, sync::Arc};

use tauri::{AppHandle, State, ipc::Response};

use crate::{
    AppState,
    database::NewProject,
    discovery, editor,
    error::{AppError, AppResult},
    model::{DiscoveryReport, EditorLaunchResult, ProjectSummary},
};

#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> AppResult<Vec<ProjectSummary>> {
    let repository = Arc::clone(&state.repository);
    tauri::async_runtime::spawn_blocking(move || repository.list_projects())
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
}

#[tauri::command]
pub async fn inspect_project(root_path: String) -> AppResult<DiscoveryReport> {
    tauri::async_runtime::spawn_blocking(move || discovery::inspect(&PathBuf::from(root_path)))
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
}

#[tauri::command]
pub async fn add_project(
    root_path: String,
    main_file: String,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let repository = Arc::clone(&state.repository);
    tauri::async_runtime::spawn_blocking(move || {
        let root = PathBuf::from(&root_path);
        if !root.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "{root_path} is not a directory"
            )));
        }
        let root = root.canonicalize()?;
        let canonical_root = root
            .to_str()
            .ok_or_else(|| AppError::InvalidInput("project path is not valid UTF-8".into()))?;
        if crate::toolchain::resolve_executable("latexmk").is_none() {
            return Err(AppError::ToolUnavailable(
                "latexmk was not found. Install a TeX distribution or add latexmk to PATH.".into(),
            ));
        }
        let main = discovery::validate_main(&root, &main_file)?;
        let engine = discovery::detect_engine(&main)?;
        let project_name = root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("LaTeX project");
        let working = main
            .parent()
            .and_then(|parent| parent.strip_prefix(&root).ok())
            .map(|relative| {
                if relative.as_os_str().is_empty() {
                    ".".into()
                } else {
                    relative.to_string_lossy().into_owned()
                }
            })
            .unwrap_or_else(|| ".".into());
        repository.add_or_update_project(NewProject {
            name: project_name,
            root_path: canonical_root,
            main_file: &main_file,
            working_directory: &working,
            engine,
        })
    })
    .await
    .map_err(|error| AppError::Task(error.to_string()))?
}

#[tauri::command]
pub async fn activate_project(
    project_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let repository = Arc::clone(&state.repository);
    let project = tauri::async_runtime::spawn_blocking(move || {
        let project = repository.get_project(project_id)?;
        if !project.root().is_dir() {
            return Err(AppError::NotFound(format!(
                "project folder is no longer available: {}",
                project.root_path
            )));
        }
        discovery::validate_main(&project.root(), &project.main_file)?;
        Ok(project)
    })
    .await
    .map_err(|error| AppError::Task(error.to_string()))??;
    state.builds.activate(app, project.clone()).await?;
    state.repository.get_project(project_id)
}

#[tauri::command]
pub async fn deactivate_project(state: State<'_, AppState>) -> AppResult<()> {
    state.builds.stop().await;
    Ok(())
}

#[tauri::command]
pub async fn rebuild_project(project_id: i64, state: State<'_, AppState>) -> AppResult<()> {
    state.builds.rebuild(project_id).await
}

#[tauri::command]
pub async fn read_project_pdf(project_id: i64, state: State<'_, AppState>) -> AppResult<Response> {
    let path = state.repository.pdf_path(project_id)?;
    let canonical = tokio::fs::canonicalize(&path).await?;
    let artifact_root = tokio::fs::canonicalize(&state.artifact_root).await?;
    if !canonical.starts_with(&artifact_root) {
        return Err(AppError::InvalidInput(
            "cached PDF path is outside Press-managed storage".into(),
        ));
    }
    let bytes = tokio::fs::read(canonical).await?;
    if bytes.len() < 5 || &bytes[..5] != b"%PDF-" {
        return Err(AppError::Build("cached artifact is not a valid PDF".into()));
    }
    Ok(Response::new(bytes))
}

#[tauri::command]
pub async fn get_build_log(project_id: i64, state: State<'_, AppState>) -> AppResult<String> {
    let path = state.builds.log_path(project_id);
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

#[tauri::command]
pub async fn launch_neovim(
    project_id: i64,
    state: State<'_, AppState>,
) -> AppResult<EditorLaunchResult> {
    let project = state.repository.get_project(project_id)?;
    tauri::async_runtime::spawn_blocking(move || editor::launch(&project))
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
}

#[tauri::command]
pub async fn editor_status(project_id: i64, state: State<'_, AppState>) -> AppResult<String> {
    let project = state.repository.get_project(project_id)?;
    tauri::async_runtime::spawn_blocking(move || editor::status(&project))
        .await
        .map_err(|error| AppError::Task(error.to_string()))
}
