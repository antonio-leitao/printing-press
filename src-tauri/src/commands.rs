use std::{path::PathBuf, sync::Arc};

use tauri::{AppHandle, State};

use crate::{
    AppState,
    database::{NewProject, ProjectEdit},
    discovery, editor,
    error::{AppError, AppResult},
    model::{
        DiscoveryReport, DocumentKind, EditorLaunchResult, Engine, OpenRequest, PageSize,
        ProjectSummary, SearchHit, SnapshotSummary, SourceRef, TextBox, VersionSummary,
    },
};

/// Every database and filesystem call goes through here, off the async runtime.
async fn blocking<T, F>(work: F) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
}

fn source_ref(value: Option<String>) -> AppResult<SourceRef> {
    match value {
        None => Ok(SourceRef::Worktree),
        Some(value) => value.parse(),
    }
}

fn engine(value: Option<String>) -> AppResult<Option<Engine>> {
    value.map(|value| value.parse()).transpose()
}

#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> AppResult<Vec<ProjectSummary>> {
    let repository = Arc::clone(&state.repository);
    blocking(move || repository.list_projects()).await
}

#[tauri::command]
pub async fn inspect_project(root_path: String) -> AppResult<DiscoveryReport> {
    blocking(move || discovery::inspect(&PathBuf::from(root_path))).await
}

/// Inspects one named file rather than a folder. Picking a file says "this
/// document, in this folder" — the only way to add markdown, and a way to add a
/// single LaTeX file without discovery ranging over its neighbours.
#[tauri::command]
pub async fn inspect_document(file_path: String) -> AppResult<DiscoveryReport> {
    blocking(move || discovery::document_report(&PathBuf::from(file_path))).await
}

#[tauri::command]
pub async fn add_project(
    root_path: String,
    main_file: String,
    engine_override: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let repository = Arc::clone(&state.repository);
    let requested = engine(engine_override)?;
    let id = blocking(move || {
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
        let kind = DocumentKind::of(&main);
        // Markdown reaches latexmk through pandoc, so both have to be present.
        if kind == DocumentKind::Markdown
            && crate::toolchain::resolve_executable("pandoc").is_none()
        {
            return Err(AppError::ToolUnavailable(
                "pandoc was not found. Install pandoc to compile markdown.".into(),
            ));
        }
        let engine = match requested {
            Some(engine) => engine,
            // A markdown document says nothing about the TeX engine; pandoc's
            // default output compiles with any of them.
            None if kind == DocumentKind::Markdown => Engine::PdfLatex,
            None => discovery::detect_engine(&main)?,
        };
        // A folder can hold several documents, and they cannot all be called
        // after it. Markdown is always one file, so it takes the file's name;
        // LaTeX keeps the folder's, which is what a paper is usually called.
        let folder_name = root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("LaTeX project");
        let project_name = match kind {
            DocumentKind::Markdown => main
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(folder_name),
            DocumentKind::Latex => folder_name,
        };
        let working = main
            .parent()
            .and_then(|parent| parent.strip_prefix(&root).ok())
            .map(|relative| {
                if relative.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    relative.to_string_lossy().into_owned()
                }
            })
            .unwrap_or_else(|| ".".to_owned());
        Ok(repository
            .upsert_project(NewProject {
                name: project_name,
                root_path: canonical_root,
                main_file: &main_file,
                working_directory: &working,
                kind,
                engine,
            })?
            .id)
    })
    .await?;

    let repository = Arc::clone(&state.repository);
    blocking(move || repository.project_summary(id)).await
}

/// Makes a project current: watches it and builds its working tree.
#[tauri::command]
pub async fn open_project(
    project_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let repository = Arc::clone(&state.repository);
    let project = blocking(move || {
        let project = repository.get_project(project_id)?;
        if !project.root().is_dir() {
            return Err(AppError::NotFound(format!(
                "the project folder is no longer available: {}",
                project.root_path
            )));
        }
        discovery::validate_main(&project.root(), &project.main_file)?;
        Ok(project)
    })
    .await?;

    Arc::clone(&state.builds).open(app, project).await?;
    let repository = Arc::clone(&state.repository);
    blocking(move || repository.project_summary(project_id)).await
}

#[tauri::command]
pub async fn close_project(state: State<'_, AppState>) -> AppResult<()> {
    state.builds.close().await;
    Ok(())
}

/// Queues a build. Defaults to the working tree; a source reference selects a
/// version from the history.
#[tauri::command]
pub async fn build_project(
    project_id: i64,
    source_ref: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<u64> {
    let target = self::source_ref(source_ref)?;
    let repository = Arc::clone(&state.repository);
    let project = blocking(move || repository.get_project(project_id)).await?;
    Arc::clone(&state.builds)
        .request(app, project, target)
        .await
}

#[tauri::command]
pub async fn rename_project(
    project_id: i64,
    name: String,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let repository = Arc::clone(&state.repository);
    blocking(move || {
        repository.update_project(
            project_id,
            ProjectEdit {
                name: Some(name),
                ..ProjectEdit::default()
            },
        )?;
        repository.project_summary(project_id)
    })
    .await
}

/// Changes the main file or the engine. Both invalidate every cached PDF, so the
/// discarded ones are deleted here.
#[tauri::command]
pub async fn update_project_settings(
    project_id: i64,
    main_file: Option<String>,
    engine_override: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let requested = engine(engine_override)?;
    let repository = Arc::clone(&state.repository);
    let main_file_for_check = main_file.clone();
    let (summary, discarded) = blocking(move || {
        let project = repository.get_project(project_id)?;
        let working = match &main_file_for_check {
            Some(main_file) => {
                let root = project.root().canonicalize()?;
                let main = discovery::validate_main(&root, main_file)?;
                let relative = main
                    .parent()
                    .and_then(|parent| parent.strip_prefix(&root).ok())
                    .filter(|relative| !relative.as_os_str().is_empty())
                    .map(|relative| relative.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ".".to_owned());
                Some(relative)
            }
            None => None,
        };
        let (_, discarded) = repository.update_project(
            project_id,
            ProjectEdit {
                name: None,
                main_file,
                working_directory: working,
                engine: requested,
            },
        )?;
        Ok((repository.project_summary(project_id)?, discarded))
    })
    .await?;

    for path in discarded {
        let _ = tokio::fs::remove_file(path).await;
    }
    // The cache is empty now, so rebuild what the user is looking at.
    let repository = Arc::clone(&state.repository);
    let project = blocking(move || repository.get_project(project_id)).await?;
    let _ = Arc::clone(&state.builds)
        .request(app, project, SourceRef::Worktree)
        .await;
    Ok(summary)
}

#[tauri::command]
pub async fn delete_project(project_id: i64, state: State<'_, AppState>) -> AppResult<()> {
    state.builds.cancel_project(project_id).await;
    let repository = Arc::clone(&state.repository);
    let orphaned = blocking(move || repository.delete_project(project_id)).await?;
    for path in orphaned {
        let _ = tokio::fs::remove_file(path).await;
    }
    state.builds.discard_project_storage(project_id).await;
    Ok(())
}

/// Stores the project's source as it is right now, under a title.
///
/// Deliberate and titled, never automatic: the editor's undo already covers
/// keystrokes, and a history worth reading is one where every entry was meant.
#[tauri::command]
pub async fn create_snapshot(
    project_id: i64,
    title: String,
    body: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<SnapshotSummary> {
    let repository = Arc::clone(&state.repository);
    let objects = state.objects_root.clone();
    let snapshot = blocking(move || {
        let project = repository.get_project(project_id)?;
        // A markdown project is one document sharing a folder with others, so
        // its history holds that document and the assets, not the neighbours.
        let scope = match project.kind {
            DocumentKind::Markdown => crate::snapshot::Scope::Document {
                main_file: &project.main_file,
            },
            DocumentKind::Latex => crate::snapshot::Scope::Folder,
        };
        let capture = crate::snapshot::capture(&project.root(), &objects, scope)?;
        repository.create_snapshot(project_id, &capture, &title, body.as_deref())
    })
    .await?;

    // Build it straight away: a version you cannot see is not much of a version.
    let repository = Arc::clone(&state.repository);
    let project = blocking(move || repository.get_project(project_id)).await?;
    let _ = Arc::clone(&state.builds)
        .request(
            app,
            project,
            SourceRef::Snapshot(snapshot.revision.clone()),
        )
        .await;
    Ok(snapshot)
}

/// The history: the working tree pinned at the top, then every snapshot.
#[tauri::command]
pub async fn list_versions(
    project_id: i64,
    state: State<'_, AppState>,
) -> AppResult<Vec<VersionSummary>> {
    let repository = Arc::clone(&state.repository);
    blocking(move || repository.list_versions(project_id)).await
}

/// Titles are interface text and can be corrected after the fact.
#[tauri::command]
pub async fn rename_snapshot(
    snapshot_id: i64,
    title: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let repository = Arc::clone(&state.repository);
    blocking(move || repository.rename_snapshot(snapshot_id, &title)).await
}

#[tauri::command]
pub async fn delete_snapshot(snapshot_id: i64, state: State<'_, AppState>) -> AppResult<()> {
    let repository = Arc::clone(&state.repository);
    let (project_id, revision, last_of_its_revision) =
        blocking(move || repository.delete_snapshot(snapshot_id)).await?;

    // Only when nothing else points at that content is its build worth dropping.
    if last_of_its_revision {
        state
            .builds
            .cancel_version(project_id, &SourceRef::Snapshot(revision.clone()))
            .await;
        let repository = Arc::clone(&state.repository);
        let orphaned = blocking(move || {
            repository.forget_version(project_id, &SourceRef::Snapshot(revision))
        })
        .await?;
        for path in orphaned {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
    Ok(())
}

/// Every page's size in PDF points, so the viewer can lay out a whole document
/// before drawing any of it.
#[tauri::command]
pub async fn page_layout(
    artifact_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<Vec<PageSize>> {
    let path = crate::protocol::resolve(&app, artifact_id).await?;
    Ok(state
        .renderer
        .geometry(path)
        .await?
        .into_iter()
        .map(|page| PageSize {
            width: page.width,
            height: page.height,
        })
        .collect())
}

/// Word boxes for one page, in PDF points, for the selection overlay.
#[tauri::command]
pub async fn page_words(
    artifact_id: i64,
    page: usize,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<Vec<TextBox>> {
    let path = crate::protocol::resolve(&app, artifact_id).await?;
    Ok(state
        .renderer
        .words(path, page)
        .await?
        .into_iter()
        .map(|word| TextBox {
            text: word.text,
            x: word.x,
            y: word.y,
            width: word.width,
            height: word.height,
            line: word.line,
        })
        .collect())
}

/// Full-document search, run by MuPDF itself.
#[tauri::command]
pub async fn search_document(
    artifact_id: i64,
    needle: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<Vec<SearchHit>> {
    if needle.trim().is_empty() {
        return Ok(Vec::new());
    }
    let path = crate::protocol::resolve(&app, artifact_id).await?;
    Ok(state
        .renderer
        .search(path, needle)
        .await?
        .into_iter()
        .map(|hit| SearchHit {
            page: hit.page,
            x: hit.x,
            y: hit.y,
            width: hit.width,
            height: hit.height,
        })
        .collect())
}

#[tauri::command]
pub async fn get_build_log(
    project_id: i64,
    source_ref: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let target = self::source_ref(source_ref)?;
    let path = state.builds.log_path(project_id, &target);
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
    let repository = Arc::clone(&state.repository);
    blocking(move || {
        let project = repository.get_project(project_id)?;
        editor::launch(&project)
    })
    .await
}

/// Collects anything Press wants to say about how it started. Taking it clears
/// it, so it is said once.
#[tauri::command]
pub async fn take_startup_notice(state: State<'_, AppState>) -> AppResult<Option<String>> {
    Ok(state
        .startup_notice
        .lock()
        .map_err(|_| AppError::Task("the startup notice was lost".into()))?
        .take())
}

/// Collects a path handed to Press from the command line, if there is one.
/// Taking it clears it, so the same request is never opened twice.
#[tauri::command]
pub async fn take_pending_open(state: State<'_, AppState>) -> AppResult<Option<OpenRequest>> {
    Ok(state
        .pending_open
        .lock()
        .map_err(|_| AppError::Task("the pending request was lost".into()))?
        .take())
}

#[tauri::command]
pub async fn editor_status(project_id: i64, state: State<'_, AppState>) -> AppResult<String> {
    let repository = Arc::clone(&state.repository);
    blocking(move || {
        let project = repository.get_project(project_id)?;
        Ok(editor::status(&project).to_owned())
    })
    .await
}
