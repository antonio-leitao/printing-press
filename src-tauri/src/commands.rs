use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use tauri::{AppHandle, Manager, State};

use crate::{
    AppState,
    database::{NewProject, ProjectEdit},
    documents, editor,
    error::{AppError, AppResult},
    model::{
        DocumentKind, EditorLaunchResult, Engine, OpenRequest, PageSize, ProjectSummary, SearchHit,
        SnapshotOutcome, SourceRef, TextBox, VersionSummary,
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

/// Resolves a path to the documents it means.
///
/// The one way in: the Add button, `:Press`, and `press <path>` all ask this,
/// because they are all the same question.
#[tauri::command]
pub async fn resolve_path(path: String, state: State<'_, AppState>) -> AppResult<OpenRequest> {
    let repository = Arc::clone(&state.repository);
    blocking(move || documents::resolve(&PathBuf::from(path), &repository)).await
}

/// Keeps one document as a project. Adding a document Press already has just
/// updates it, so this is also how a candidate is re-opened.
#[tauri::command]
pub async fn add_project(
    document_path: String,
    name: Option<String>,
    engine_override: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let repository = Arc::clone(&state.repository);
    let requested = engine(engine_override)?;
    let id = blocking(move || {
        let document = documents::validate(&PathBuf::from(&document_path))?;
        let canonical = document
            .to_str()
            .ok_or_else(|| AppError::InvalidInput("that path is not valid UTF-8".into()))?;
        if crate::toolchain::resolve_executable("latexmk").is_none() {
            return Err(AppError::ToolUnavailable(
                "latexmk was not found. Install a TeX distribution or add latexmk to PATH.".into(),
            ));
        }
        // Markdown reaches latexmk through pandoc, so both have to be present.
        if DocumentKind::of(&document) == DocumentKind::Markdown
            && crate::toolchain::resolve_executable("pandoc").is_none()
        {
            return Err(AppError::ToolUnavailable(
                "pandoc was not found. Install pandoc to compile markdown.".into(),
            ));
        }
        // A markdown document says nothing about the TeX engine, and pandoc's
        // output compiles with any of them, so pdflatex is the honest default.
        let engine = requested
            .or_else(|| documents::detect_engine(&document))
            .unwrap_or(Engine::PdfLatex);
        let suggested = name.unwrap_or_else(|| canonical.to_owned());
        Ok(repository
            .upsert_project(NewProject {
                name: &suggested,
                document_path: canonical,
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
        documents::validate(&project.document())?;
        Ok(project)
    })
    .await?;

    Arc::clone(&state.builds).open(app, project).await?;
    let repository = Arc::clone(&state.repository);
    blocking(move || repository.project_summary(project_id)).await
}

/// Shows a PDF without keeping it.
///
/// Not a project: a project is a source document, and everything Press stores
/// about one — its history, its builds, its engine — follows from the document
/// it compiles. A PDF has none of that behind it, so it stays out of the
/// library entirely. It is watched while it is open, because whatever is
/// rebuilding it is not Press.
#[tauri::command]
pub async fn open_pdf(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<crate::model::LooseDocument> {
    let viewing = Arc::clone(&state.viewing);
    blocking(move || viewing.open(&app, &PathBuf::from(path))).await
}

#[tauri::command]
pub async fn close_pdf(document_id: i64, state: State<'_, AppState>) -> AppResult<()> {
    state.viewing.close(document_id)
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

/// Pins a project to the top of the library, or unpins it. Nothing else changes,
/// so no cached build is affected.
#[tauri::command]
pub async fn set_project_pinned(
    project_id: i64,
    pinned: bool,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let repository = Arc::clone(&state.repository);
    blocking(move || {
        repository.update_project(
            project_id,
            ProjectEdit {
                pinned: Some(pinned),
                ..ProjectEdit::default()
            },
        )?;
        repository.project_summary(project_id)
    })
    .await
}

/// Changes the engine, which invalidates every cached PDF, so the discarded ones
/// are deleted here.
///
/// The document itself is not settable: a different document is a different
/// project, which is what makes it possible to keep several in one folder.
#[tauri::command]
pub async fn set_project_engine(
    project_id: i64,
    engine_override: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<ProjectSummary> {
    let requested = engine(Some(engine_override))?;
    let repository = Arc::clone(&state.repository);
    let (summary, discarded) = blocking(move || {
        let (_, discarded) = repository.update_project(
            project_id,
            ProjectEdit {
                engine: requested,
                ..ProjectEdit::default()
            },
        )?;
        Ok((repository.project_summary(project_id)?, discarded))
    })
    .await?;

    for path in discarded {
        crate::runner::discard_publication(&path).await;
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
        crate::runner::discard_publication(&path).await;
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
) -> AppResult<SnapshotOutcome> {
    let repository = Arc::clone(&state.repository);
    let objects = state.objects_root.clone();
    let outcome = blocking(move || {
        let project = repository.get_project(project_id)?;
        let directory = project.directory();
        // A document's history holds its directory minus the documents that are
        // other projects: their drafts are not this document's versions.
        let foreign = repository
            .foreign_documents(project_id, &directory)?
            .into_iter()
            .collect::<HashSet<_>>();
        let capture = crate::snapshot::capture(&directory, &objects, &foreign)?;
        repository.create_snapshot(project_id, &capture, &title, body.as_deref())
    })
    .await?;

    // Nothing was stored, so there is nothing to build: the version this
    // content already is was built when it was stored.
    let SnapshotOutcome::Stored { snapshot } = &outcome else {
        return Ok(outcome);
    };

    // Build it straight away: a version you cannot see is not much of a version.
    let repository = Arc::clone(&state.repository);
    let project = blocking(move || repository.get_project(project_id)).await?;
    let _ = Arc::clone(&state.builds)
        .request(app, project, SourceRef::Snapshot(snapshot.revision.clone()))
        .await;
    Ok(outcome)
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
        let orphaned =
            blocking(move || repository.forget_version(project_id, &SourceRef::Snapshot(revision)))
                .await?;
        for path in orphaned {
            crate::runner::discard_publication(&path).await;
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

/// The links on one page: references and citations that lead inside the
/// document, and addresses that lead out of it.
#[tauri::command]
pub async fn page_links(
    artifact_id: i64,
    page: usize,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<Vec<crate::model::LinkBox>> {
    let path = crate::protocol::resolve(&app, artifact_id).await?;
    Ok(state
        .renderer
        .links(path, page)
        .await?
        .into_iter()
        .map(|link| crate::model::LinkBox {
            x: link.x,
            y: link.y,
            width: link.width,
            height: link.height,
            page: link.page,
            top: link.top,
            uri: link.uri,
        })
        .collect())
}

/// Opens a link that leads out of the document, in whatever the system uses for
/// it.
///
/// Only the schemes a document has any business carrying. A PDF is a file from
/// somewhere else, and `file:` — never mind anything stranger — would make one
/// click enough to reach anything Press can read.
#[tauri::command]
pub async fn open_external(uri: String) -> AppResult<()> {
    let allowed = ["http://", "https://", "mailto:"];
    if !allowed.iter().any(|scheme| uri.starts_with(scheme)) {
        return Err(AppError::InvalidInput(format!(
            "Press only opens web and mail links. This one points at {}.",
            uri.split(':').next().unwrap_or("something else")
        )));
    }
    blocking(move || {
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(all(unix, not(target_os = "macos")))]
        let opener = "xdg-open";
        #[cfg(windows)]
        let opener = "explorer";
        std::process::Command::new(opener)
            .arg(&uri)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| AppError::Build(format!("could not open the link: {error}")))?;
        Ok(())
    })
    .await
}

/// The source behind a point in a built PDF.
///
/// Read-only, and for that reason it works on a stored version as well as on
/// the working tree: a snapshot's source comes out of the object store, and it
/// is guaranteed to be the source this PDF was built from, which the working
/// tree cannot promise once it has been edited.
///
/// `x` and `y` are PDF points from the top left of the page, which is what
/// SyncTeX means by a position and what the viewer already works in.
#[tauri::command]
pub async fn peek_source(
    artifact_id: i64,
    page: u32,
    x: f64,
    y: f64,
    state: State<'_, AppState>,
) -> AppResult<Option<crate::model::SourcePeek>> {
    let repository = Arc::clone(&state.repository);
    let objects = state.objects_root.clone();
    // A PDF Press only shows can still be clicked through, because latexmk
    // leaves its sync data beside whatever it built. Everything the resolver
    // needs of a project in that case is the directory the file sits in, so it
    // is given one that says exactly that and nothing more.
    let loose = state.viewing.path(artifact_id);
    blocking(move || {
        let (project, stored) = match loose {
            Some(path) => (
                crate::peek::beside(&path),
                crate::peek::loose(artifact_id, path),
            ),
            None => {
                let stored = repository.artifact(artifact_id)?;
                (repository.get_project(stored.summary.project_id)?, stored)
            }
        };
        crate::peek::resolve(&project, &stored, &repository, &objects, page, x, y)
    })
    .await
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

/// Copies a built PDF into the user's Downloads folder, and says where it went.
///
/// The only thing Press writes outside its own storage, and only when asked for
/// it by name. The source is resolved through the same check the viewer uses, so
/// a PDF outside Press-managed storage is refused here too.
#[tauri::command]
pub async fn export_artifact(
    artifact_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let source = crate::protocol::resolve(&app, artifact_id).await?;

    let repository = Arc::clone(&state.repository);
    let stem = blocking(move || {
        let stored = repository.artifact(artifact_id)?;
        let project = repository.get_project(stored.summary.project_id)?;
        // The working tree is the document itself; a snapshot carries its title
        // so several exported versions do not collide.
        let title = match &stored.summary.source_ref {
            SourceRef::Worktree => None,
            reference => repository
                .list_versions(project.id)?
                .into_iter()
                .find(|version| &version.source_ref == reference)
                .map(|version| version.title),
        };
        Ok(match title.as_deref().map(file_safe) {
            Some(title) => format!("{}-{title}", project.job_name()),
            None => project.job_name(),
        })
    })
    .await?;

    let downloads = app.path().download_dir().map_err(|error| {
        AppError::NotFound(format!(
            "Press could not find your Downloads folder: {error}"
        ))
    })?;
    let destination = unused_path(&downloads, &stem);
    tokio::fs::copy(&source, &destination)
        .await
        .map_err(|error| {
            AppError::Io(std::io::Error::new(
                error.kind(),
                format!("could not write to Downloads: {error}"),
            ))
        })?;
    Ok(destination.to_string_lossy().into_owned())
}

/// A version title is free text, so it is reduced to something that survives
/// being a file name on any platform.
fn file_safe(title: &str) -> String {
    let mut out = String::new();
    for character in title.chars().take(60) {
        if character.is_alphanumeric() {
            out.push(character);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "version".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Never overwrites: exporting the same version twice leaves both files.
fn unused_path(directory: &Path, stem: &str) -> PathBuf {
    let first = directory.join(format!("{stem}.pdf"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|suffix| directory.join(format!("{stem}-{suffix}.pdf")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
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
/// Whether a path is on its way to becoming an open request. Asked once at
/// startup, so that the library is not put up in front of a document that is
/// still being resolved.
#[tauri::command]
pub async fn expecting_open(state: State<'_, AppState>) -> AppResult<bool> {
    Ok(state
        .expecting_open
        .load(std::sync::atomic::Ordering::SeqCst)
        > 0)
}

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
