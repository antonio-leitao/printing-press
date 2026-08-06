use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::{
    error::{AppError, AppResult},
    model::{
        ArtifactSummary, BuildState, Diagnostic, DocumentKind, Engine, Project, ProjectSummary,
        SnapshotSummary, SourceRef, VersionSummary,
    },
};

pub struct Repository {
    connection: Mutex<Connection>,
    /// Something the user should know about how this database was opened.
    notice: Option<String>,
}

pub struct NewProject<'a> {
    pub name: &'a str,
    pub root_path: &'a str,
    pub main_file: &'a str,
    pub working_directory: &'a str,
    pub kind: DocumentKind,
    pub engine: Engine,
}

#[derive(Default)]
pub struct ProjectEdit {
    pub name: Option<String>,
    pub main_file: Option<String>,
    pub working_directory: Option<String>,
    pub engine: Option<Engine>,
}

pub struct NewArtifact<'a> {
    pub project_id: i64,
    pub source_ref: &'a SourceRef,
    pub engine: Engine,
    pub pdf_path: &'a Path,
    pub page_count: Option<i64>,
    pub byte_size: i64,
}

pub struct ProjectLocation {
    pub id: i64,
    pub root_path: String,
    pub main_file: String,
    pub last_opened_at: i64,
}

/// A stored artifact plus the on-disk location, which never leaves the backend.
pub struct StoredArtifact {
    pub summary: ArtifactSummary,
    pub pdf_path: PathBuf,
}

impl Repository {
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut notice = None;

        // A database from a different schema is set aside rather than migrated,
        // and rather than refused. Refusing it meant the application could not
        // start at all, which turns a one-line explanation into a crash; and
        // deleting it would throw away someone's history to save a rename.
        if let Some(previous) = retire_incompatible(path)? {
            notice = Some(format!(
                "This database was written by a different version of Press, so it was set aside \
                 as {} and a new one started. Your projects and versions are in that file if you \
                 need them.",
                previous.display()
            ));
        }

        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        initialize(&connection)?;

        let repository = Self {
            connection: Mutex::new(connection),
            notice,
        };
        repository.mark_running_builds_interrupted()?;
        Ok(repository)
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    // -- projects ---------------------------------------------------------

    pub fn list_projects(&self) -> AppResult<Vec<ProjectSummary>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(&summary_query(""))?;
        let rows = statement.query_map([SourceRef::Worktree.to_string()], map_summary)?;
        rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            .map_err(AppError::from)?
            .into_iter()
            .collect::<AppResult<Vec<_>>>()
    }

    pub fn project_summary(&self, id: i64) -> AppResult<ProjectSummary> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(&summary_query(" WHERE p.id = ?2"))?;
        statement
            .query_row(params![SourceRef::Worktree.to_string(), id], map_summary)
            .optional()?
            .ok_or_else(|| AppError::NotFound(format!("project {id} does not exist")))?
    }

    pub fn get_project(&self, id: i64) -> AppResult<Project> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT id, name, root_path, main_file, working_directory, kind, engine,
                        created_at, last_opened_at
                 FROM projects WHERE id = ?1",
                [id],
                map_project,
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound(format!("project {id} does not exist")))?
    }

    pub fn upsert_project(&self, project: NewProject<'_>) -> AppResult<Project> {
        let now = unix_timestamp();
        let id = {
            let connection = self.lock()?;
            // A re-added project keeps its stored name: the folder name is only a
            // default for first insert, and renaming is a user action.
            connection.execute(
                "INSERT INTO projects (
                    name, root_path, main_file, working_directory, kind, engine,
                    created_at, last_opened_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                 ON CONFLICT(root_path, main_file) DO UPDATE SET
                    working_directory = excluded.working_directory,
                    kind = excluded.kind,
                    engine = excluded.engine,
                    last_opened_at = excluded.last_opened_at",
                params![
                    project.name,
                    project.root_path,
                    project.main_file,
                    project.working_directory,
                    project.kind.as_token(),
                    project.engine.as_token(),
                    now
                ],
            )?;
            connection.query_row(
                "SELECT id FROM projects WHERE root_path = ?1 AND main_file = ?2",
                params![project.root_path, project.main_file],
                |row| row.get::<_, i64>(0),
            )?
        };
        self.get_project(id)
    }

    /// Applies a partial edit. Changing the engine discards every artifact and
    /// build state for the project: versions compiled by different engines are
    /// not comparable, so keeping them would poison the cache.
    pub fn update_project(&self, id: i64, edit: ProjectEdit) -> AppResult<(Project, Vec<PathBuf>)> {
        let current = self.get_project(id)?;
        let engine_changed = edit.engine.is_some_and(|engine| engine != current.engine);
        let discarded = {
            let mut connection = self.lock()?;
            let transaction = connection.transaction()?;
            if let Some(name) = edit.name.as_deref() {
                let name = name.trim();
                if name.is_empty() {
                    return Err(AppError::InvalidInput("a project needs a name".into()));
                }
                transaction.execute(
                    "UPDATE projects SET name = ?2 WHERE id = ?1",
                    params![id, name],
                )?;
            }
            if let Some(main_file) = edit.main_file.as_deref() {
                transaction.execute(
                    "UPDATE projects SET main_file = ?2 WHERE id = ?1",
                    params![id, main_file],
                )?;
            }
            if let Some(working) = edit.working_directory.as_deref() {
                transaction.execute(
                    "UPDATE projects SET working_directory = ?2 WHERE id = ?1",
                    params![id, working],
                )?;
            }
            if let Some(engine) = edit.engine {
                transaction.execute(
                    "UPDATE projects SET engine = ?2 WHERE id = ?1",
                    params![id, engine.as_token()],
                )?;
            }
            let discarded = if engine_changed || edit.main_file.is_some() {
                let paths = artifact_paths_for_project(&transaction, id)?;
                transaction.execute("DELETE FROM artifacts WHERE project_id = ?1", [id])?;
                transaction.execute("DELETE FROM build_states WHERE project_id = ?1", [id])?;
                paths
            } else {
                Vec::new()
            };
            transaction.commit()?;
            discarded
        };
        Ok((self.get_project(id)?, discarded))
    }

    /// Removes the project and returns every PDF that is now unreferenced, for
    /// the caller to delete once the transaction has committed.
    pub fn delete_project(&self, id: i64) -> AppResult<Vec<PathBuf>> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let paths = artifact_paths_for_project(&transaction, id)?;
        let removed = transaction.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        if removed == 0 {
            return Err(AppError::NotFound(format!("project {id} does not exist")));
        }
        transaction.execute("DELETE FROM artifacts WHERE project_id = ?1", [id])?;
        transaction.execute("DELETE FROM build_states WHERE project_id = ?1", [id])?;
        transaction.commit()?;
        Ok(paths)
    }

    pub fn touch_project(&self, id: i64) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE projects SET last_opened_at = ?2 WHERE id = ?1",
            params![id, unix_timestamp()],
        )?;
        Ok(())
    }

    // -- build state ------------------------------------------------------

    pub fn build_state(&self, project_id: i64, source_ref: &SourceRef) -> AppResult<BuildState> {
        let connection = self.lock()?;
        let state = connection
            .query_row(
                "SELECT source_ref, status, started_at, finished_at, duration_ms,
                        error_summary, diagnostics
                 FROM build_states WHERE project_id = ?1 AND source_ref = ?2",
                params![project_id, source_ref.to_string()],
                map_build_state,
            )
            .optional()?;
        match state {
            Some(state) => state,
            None => Ok(BuildState::never(source_ref.clone())),
        }
    }

    pub fn set_build_state(&self, project_id: i64, state: &BuildState) -> AppResult<()> {
        let diagnostics = serde_json::to_string(&state.diagnostics)?;
        self.lock()?.execute(
            "INSERT INTO build_states (
                project_id, source_ref, status, started_at, finished_at,
                duration_ms, error_summary, diagnostics
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(project_id, source_ref) DO UPDATE SET
                status = excluded.status,
                started_at = excluded.started_at,
                finished_at = excluded.finished_at,
                duration_ms = excluded.duration_ms,
                error_summary = excluded.error_summary,
                diagnostics = excluded.diagnostics",
            params![
                project_id,
                state.source_ref.to_string(),
                state.status.as_token(),
                state.started_at,
                state.finished_at,
                state.duration_ms,
                state.error_summary,
                diagnostics,
            ],
        )?;
        Ok(())
    }

    /// Builds cannot survive a restart, so anything still marked queued or
    /// running belongs to a process that is gone.
    pub fn mark_running_builds_interrupted(&self) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE build_states
                SET status = 'interrupted',
                    error_summary = 'The previous build ended when Press closed.',
                    finished_at = ?1
              WHERE status IN ('queued', 'running')",
            [unix_timestamp()],
        )?;
        Ok(())
    }

    // -- artifacts --------------------------------------------------------

    pub fn artifact(&self, artifact_id: i64) -> AppResult<StoredArtifact> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT id, project_id, source_ref, engine, pdf_path, page_count,
                        byte_size, built_at, revision
                 FROM artifacts WHERE id = ?1",
                [artifact_id],
                map_artifact,
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound(format!("artifact {artifact_id} does not exist")))?
    }

    pub fn artifact_for(
        &self,
        project_id: i64,
        source_ref: &SourceRef,
        engine: Engine,
    ) -> AppResult<Option<StoredArtifact>> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT id, project_id, source_ref, engine, pdf_path, page_count,
                        byte_size, built_at, revision
                 FROM artifacts
                 WHERE project_id = ?1 AND source_ref = ?2 AND engine = ?3",
                params![project_id, source_ref.to_string(), engine.as_token()],
                map_artifact,
            )
            .optional()?
            .transpose()
    }

    /// Publishes a built PDF. The working tree's artifact is replaced in place
    /// and its revision bumped; a snapshot's artifact is written once and then
    /// never needs invalidating. Returns the superseded file, if any.
    pub fn record_artifact(
        &self,
        artifact: NewArtifact<'_>,
    ) -> AppResult<(ArtifactSummary, Option<PathBuf>)> {
        let pdf_path = artifact
            .pdf_path
            .to_str()
            .ok_or_else(|| AppError::InvalidInput("PDF path is not valid UTF-8".into()))?;
        let source_token = artifact.source_ref.to_string();
        let now = unix_timestamp();

        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT id, pdf_path, revision FROM artifacts
                 WHERE project_id = ?1 AND source_ref = ?2 AND engine = ?3",
                params![
                    artifact.project_id,
                    &source_token,
                    artifact.engine.as_token()
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;

        let (id, revision, superseded) = match existing {
            Some((id, previous_path, revision)) => {
                let revision = revision + 1;
                transaction.execute(
                    "UPDATE artifacts SET pdf_path = ?2, page_count = ?3, byte_size = ?4,
                            built_at = ?5, revision = ?6
                     WHERE id = ?1",
                    params![
                        id,
                        pdf_path,
                        artifact.page_count,
                        artifact.byte_size,
                        now,
                        revision
                    ],
                )?;
                let superseded = (previous_path != pdf_path).then(|| PathBuf::from(previous_path));
                (id, revision, superseded)
            }
            None => {
                transaction.execute(
                    "INSERT INTO artifacts (
                        project_id, source_ref, engine, pdf_path, page_count,
                        byte_size, built_at, revision
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
                    params![
                        artifact.project_id,
                        &source_token,
                        artifact.engine.as_token(),
                        pdf_path,
                        artifact.page_count,
                        artifact.byte_size,
                        now,
                    ],
                )?;
                (transaction.last_insert_rowid(), 1, None)
            }
        };
        transaction.commit()?;

        Ok((
            ArtifactSummary {
                id,
                project_id: artifact.project_id,
                source_ref: artifact.source_ref.clone(),
                engine: artifact.engine,
                page_count: artifact.page_count,
                byte_size: artifact.byte_size,
                built_at: now,
                revision,
            },
            superseded,
        ))
    }

    // -- snapshots --------------------------------------------------------

    /// Records a captured snapshot. The objects are already in the store; this
    /// is what makes them findable again.
    pub fn create_snapshot(
        &self,
        project_id: i64,
        capture: &crate::snapshot::Capture,
        title: &str,
        body: Option<&str>,
    ) -> AppResult<SnapshotSummary> {
        let title = title.trim();
        if title.is_empty() {
            return Err(AppError::InvalidInput("a version needs a title".into()));
        }
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(AppError::InvalidInput(format!(
                "a title has to fit in {MAX_TITLE_CHARS} characters"
            )));
        }
        let body = body.map(str::trim).filter(|body| !body.is_empty());
        let now = unix_timestamp();

        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO snapshots (
                project_id, revision, title, body, created_at, file_count, byte_size
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                project_id,
                capture.revision,
                title,
                body,
                now,
                capture.files.len() as i64,
                capture.byte_size,
            ],
        )?;
        let id = transaction.last_insert_rowid();
        {
            let mut statement = transaction.prepare(
                "INSERT INTO snapshot_files (snapshot_id, path, object, byte_size)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for file in &capture.files {
                statement.execute(params![id, file.path, file.object, file.byte_size])?;
            }
        }
        transaction.commit()?;

        Ok(SnapshotSummary {
            id,
            project_id,
            revision: capture.revision.clone(),
            title: title.to_owned(),
            body: body.map(ToOwned::to_owned),
            created_at: now,
            file_count: capture.files.len() as i64,
            byte_size: capture.byte_size,
        })
    }

    pub fn list_snapshots(&self, project_id: i64) -> AppResult<Vec<SnapshotSummary>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, project_id, revision, title, body, created_at, file_count, byte_size
             FROM snapshots WHERE project_id = ?1
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = statement.query_map([project_id], map_snapshot)?;
        rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            .map_err(AppError::from)
    }

    /// The files a revision holds. Any snapshot with that revision will do:
    /// identical content is what the revision means.
    pub fn snapshot_manifest(
        &self,
        project_id: i64,
        revision: &str,
    ) -> AppResult<Vec<crate::snapshot::StoredFile>> {
        let connection = self.lock()?;
        let snapshot_id: Option<i64> = connection
            .query_row(
                "SELECT id FROM snapshots WHERE project_id = ?1 AND revision = ?2
                 ORDER BY created_at ASC LIMIT 1",
                params![project_id, revision],
                |row| row.get(0),
            )
            .optional()?;
        let snapshot_id = snapshot_id.ok_or_else(|| {
            AppError::NotFound(format!("no stored version matches revision {revision}"))
        })?;

        let mut statement = connection.prepare(
            "SELECT path, object, byte_size FROM snapshot_files
             WHERE snapshot_id = ?1 ORDER BY path",
        )?;
        let rows = statement.query_map([snapshot_id], |row| {
            Ok(crate::snapshot::StoredFile {
                path: row.get(0)?,
                object: row.get(1)?,
                byte_size: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            .map_err(AppError::from)
    }

    pub fn rename_snapshot(&self, snapshot_id: i64, title: &str) -> AppResult<()> {
        let title = title.trim();
        if title.is_empty() {
            return Err(AppError::InvalidInput("a version needs a title".into()));
        }
        let changed = self.lock()?.execute(
            "UPDATE snapshots SET title = ?2 WHERE id = ?1",
            params![snapshot_id, title],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound(format!(
                "version {snapshot_id} does not exist"
            )));
        }
        Ok(())
    }

    /// Removes a snapshot. Returns the revision, so the caller can drop any
    /// artifact built from it when nothing else refers to that revision.
    pub fn delete_snapshot(&self, snapshot_id: i64) -> AppResult<(i64, String, bool)> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let (project_id, revision): (i64, String) = transaction
            .query_row(
                "SELECT project_id, revision FROM snapshots WHERE id = ?1",
                [snapshot_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound(format!("version {snapshot_id} does not exist")))?;
        transaction.execute("DELETE FROM snapshot_files WHERE snapshot_id = ?1", [snapshot_id])?;
        transaction.execute("DELETE FROM snapshots WHERE id = ?1", [snapshot_id])?;
        let remaining: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM snapshots WHERE project_id = ?1 AND revision = ?2",
            params![project_id, &revision],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok((project_id, revision, remaining == 0))
    }

    /// The history, newest first, with the working tree pinned at the top.
    /// Each row carries what Press knows about building that version.
    pub fn list_versions(&self, project_id: i64) -> AppResult<Vec<VersionSummary>> {
        let project = self.get_project(project_id)?;
        let mut versions = vec![VersionSummary {
            source_ref: SourceRef::Worktree,
            title: "Working tree".into(),
            snapshot: None,
            build: self.build_state(project_id, &SourceRef::Worktree)?,
            artifact: self
                .artifact_for(project_id, &SourceRef::Worktree, project.engine)?
                .map(|stored| stored.summary),
        }];

        for snapshot in self.list_snapshots(project_id)? {
            let source_ref = SourceRef::Snapshot(snapshot.revision.clone());
            versions.push(VersionSummary {
                title: snapshot.title.clone(),
                build: self.build_state(project_id, &source_ref)?,
                artifact: self
                    .artifact_for(project_id, &source_ref, project.engine)?
                    .map(|stored| stored.summary),
                snapshot: Some(snapshot),
                source_ref,
            });
        }
        Ok(versions)
    }

    /// Drops one version's cached build. Returns the PDFs to delete.
    pub fn forget_version(
        &self,
        project_id: i64,
        source_ref: &SourceRef,
    ) -> AppResult<Vec<PathBuf>> {
        let token = source_ref.to_string();
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let paths = {
            let mut statement = transaction.prepare(
                "SELECT pdf_path FROM artifacts WHERE project_id = ?1 AND source_ref = ?2",
            )?;
            let rows = statement.query_map(params![project_id, &token], |row| {
                row.get::<_, String>(0)
            })?;
            rows.filter_map(Result::ok)
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        };
        transaction.execute(
            "DELETE FROM artifacts WHERE project_id = ?1 AND source_ref = ?2",
            params![project_id, &token],
        )?;
        transaction.execute(
            "DELETE FROM build_states WHERE project_id = ?1 AND source_ref = ?2",
            params![project_id, &token],
        )?;
        transaction.commit()?;
        Ok(paths)
    }

    /// Every content hash still referenced by some snapshot, for sweeping the
    /// object store.
    pub fn referenced_objects(&self) -> AppResult<Vec<String>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT DISTINCT object FROM snapshot_files")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            .map_err(AppError::from)
    }

    pub fn managed_pdf_paths(&self) -> AppResult<Vec<PathBuf>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT pdf_path FROM artifacts")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| row.map(PathBuf::from))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Enough of each project to work out which one a path belongs to: the
    /// folder, the document inside it, and how recently it was opened.
    pub fn project_locations(&self) -> AppResult<Vec<ProjectLocation>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT id, root_path, main_file, last_opened_at FROM projects")?;
        let rows = statement.query_map([], |row| {
            Ok(ProjectLocation {
                id: row.get(0)?,
                root_path: row.get(1)?,
                main_file: row.get(2)?,
                last_opened_at: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            .map_err(AppError::from)
    }

    pub fn project_ids(&self) -> AppResult<Vec<i64>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT id FROM projects")?;
        let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn lock(&self) -> AppResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| AppError::Task("database lock was poisoned".into()))
    }
}

const SUMMARY_QUERY_BASE: &str = "
    SELECT p.id, p.name, p.root_path, p.main_file, p.working_directory, p.kind, p.engine,
           p.created_at, p.last_opened_at,
           b.source_ref AS state_source_ref, b.status, b.started_at, b.finished_at,
           b.duration_ms, b.error_summary, b.diagnostics,
           a.id AS artifact_id, a.project_id AS artifact_project_id,
           a.source_ref AS artifact_source_ref, a.engine AS artifact_engine,
           a.page_count, a.byte_size, a.built_at, a.revision
      FROM projects p
      LEFT JOIN build_states b ON b.project_id = p.id AND b.source_ref = ?1
      LEFT JOIN artifacts a ON a.project_id = p.id AND a.source_ref = ?1
                           AND a.engine = p.engine
";

const SUMMARY_ORDER: &str = " ORDER BY p.last_opened_at DESC, p.name COLLATE NOCASE";

fn summary_query(filter: &str) -> String {
    format!("{SUMMARY_QUERY_BASE}{filter}{SUMMARY_ORDER}")
}

fn artifact_paths_for_project(
    transaction: &Transaction<'_>,
    project_id: i64,
) -> AppResult<Vec<PathBuf>> {
    let mut statement =
        transaction.prepare("SELECT pdf_path FROM artifacts WHERE project_id = ?1")?;
    let rows = statement.query_map([project_id], |row| row.get::<_, String>(0))?;
    Ok(rows
        .filter_map(Result::ok)
        .map(PathBuf::from)
        .collect::<Vec<_>>())
}

/// Titles become the history sidebar, read far more often than written, so they
/// are kept to a length that stays scannable.
const MAX_TITLE_CHARS: usize = 100;

fn map_snapshot(row: &Row<'_>) -> rusqlite::Result<SnapshotSummary> {
    Ok(SnapshotSummary {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        revision: row.get("revision")?,
        title: row.get("title")?,
        body: row.get("body")?,
        created_at: row.get("created_at")?,
        file_count: row.get("file_count")?,
        byte_size: row.get("byte_size")?,
    })
}

fn map_project(row: &Row<'_>) -> rusqlite::Result<AppResult<Project>> {
    let engine: String = row.get("engine")?;
    let kind: String = row.get("kind")?;
    Ok((|| {
        Ok(Project {
            id: row_value(row, "id")?,
            name: row_value(row, "name")?,
            root_path: row_value(row, "root_path")?,
            main_file: row_value(row, "main_file")?,
            working_directory: row_value(row, "working_directory")?,
            kind: kind.parse()?,
            engine: engine.parse()?,
            created_at: row_value(row, "created_at")?,
            last_opened_at: row_value(row, "last_opened_at")?,
        })
    })())
}

fn map_build_state(row: &Row<'_>) -> rusqlite::Result<AppResult<BuildState>> {
    let source_ref: String = row.get("source_ref")?;
    let status: String = row.get("status")?;
    let diagnostics: String = row.get("diagnostics")?;
    let started_at = row.get("started_at")?;
    let finished_at = row.get("finished_at")?;
    let duration_ms = row.get("duration_ms")?;
    let error_summary = row.get("error_summary")?;
    Ok((|| {
        Ok(BuildState {
            source_ref: source_ref.parse()?,
            status: status.parse()?,
            started_at,
            finished_at,
            duration_ms,
            error_summary,
            diagnostics: serde_json::from_str::<Vec<Diagnostic>>(&diagnostics)
                .unwrap_or_default(),
        })
    })())
}

fn map_artifact(row: &Row<'_>) -> rusqlite::Result<AppResult<StoredArtifact>> {
    let source_ref: String = row.get("source_ref")?;
    let engine: String = row.get("engine")?;
    let pdf_path: String = row.get("pdf_path")?;
    let id = row.get("id")?;
    let project_id = row.get("project_id")?;
    let page_count = row.get("page_count")?;
    let byte_size = row.get("byte_size")?;
    let built_at = row.get("built_at")?;
    let revision = row.get("revision")?;
    Ok((|| {
        Ok(StoredArtifact {
            summary: ArtifactSummary {
                id,
                project_id,
                source_ref: source_ref.parse()?,
                engine: engine.parse()?,
                page_count,
                byte_size,
                built_at,
                revision,
            },
            pdf_path: PathBuf::from(pdf_path),
        })
    })())
}

fn map_summary(row: &Row<'_>) -> rusqlite::Result<AppResult<ProjectSummary>> {
    let project = map_project(row)?;
    let status: Option<String> = row.get("status")?;
    let state_source_ref: Option<String> = row.get("state_source_ref")?;
    let diagnostics: Option<String> = row.get("diagnostics")?;
    let started_at = row.get("started_at")?;
    let finished_at = row.get("finished_at")?;
    let duration_ms = row.get("duration_ms")?;
    let error_summary = row.get("error_summary")?;

    let artifact_id: Option<i64> = row.get("artifact_id")?;
    let artifact_project_id: Option<i64> = row.get("artifact_project_id")?;
    let artifact_source_ref: Option<String> = row.get("artifact_source_ref")?;
    let artifact_engine: Option<String> = row.get("artifact_engine")?;
    let page_count = row.get("page_count")?;
    let byte_size: Option<i64> = row.get("byte_size")?;
    let built_at: Option<i64> = row.get("built_at")?;
    let revision: Option<i64> = row.get("revision")?;

    Ok((|| {
        let project = project?;
        let build = match (state_source_ref, status) {
            (Some(source_ref), Some(status)) => BuildState {
                source_ref: source_ref.parse()?,
                status: status.parse()?,
                started_at,
                finished_at,
                duration_ms,
                error_summary,
                diagnostics: diagnostics
                    .as_deref()
                    .and_then(|value| serde_json::from_str::<Vec<Diagnostic>>(value).ok())
                    .unwrap_or_default(),
            },
            _ => BuildState::never(SourceRef::Worktree),
        };
        let artifact = match (
            artifact_id,
            artifact_project_id,
            artifact_source_ref,
            artifact_engine,
        ) {
            (Some(id), Some(project_id), Some(source_ref), Some(engine)) => Some(ArtifactSummary {
                id,
                project_id,
                source_ref: source_ref.parse()?,
                engine: engine.parse()?,
                page_count,
                byte_size: byte_size.unwrap_or_default(),
                built_at: built_at.unwrap_or_default(),
                revision: revision.unwrap_or(1),
            }),
            _ => None,
        };
        let root = Path::new(&project.root_path);
        let path_available = root.is_dir();
        let main_file_available = path_available && root.join(&project.main_file).is_file();
        Ok(ProjectSummary {
            project,
            build,
            artifact,
            path_available,
            main_file_available,
        })
    })())
}

fn row_value<T: rusqlite::types::FromSql>(row: &Row<'_>, name: &str) -> AppResult<T> {
    row.get(name).map_err(AppError::from)
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// -- schema ---------------------------------------------------------------

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS projects (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        root_path TEXT NOT NULL,
        main_file TEXT NOT NULL,
        working_directory TEXT NOT NULL,
        kind TEXT NOT NULL DEFAULT 'latex',
        engine TEXT NOT NULL DEFAULT 'pdflatex',
        created_at INTEGER NOT NULL,
        last_opened_at INTEGER NOT NULL,
        -- A project is a document, not a folder. Several documents can share a
        -- directory: two markdown essays side by side, or a paper and its
        -- poster. Keying on the folder alone made the second one overwrite the
        -- first.
        UNIQUE(root_path, main_file)
    );
    CREATE INDEX IF NOT EXISTS projects_last_opened_idx
        ON projects(last_opened_at DESC);

    CREATE TABLE IF NOT EXISTS artifacts (
        id INTEGER PRIMARY KEY,
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        source_ref TEXT NOT NULL,
        engine TEXT NOT NULL,
        pdf_path TEXT NOT NULL,
        page_count INTEGER,
        byte_size INTEGER NOT NULL DEFAULT 0,
        built_at INTEGER NOT NULL,
        revision INTEGER NOT NULL DEFAULT 1,
        UNIQUE(project_id, source_ref, engine)
    );
    CREATE INDEX IF NOT EXISTS artifacts_project_idx ON artifacts(project_id);

    CREATE TABLE IF NOT EXISTS snapshots (
        id INTEGER PRIMARY KEY,
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        -- Hash of the manifest. Not unique: two snapshots of identical content
        -- share a revision, and so share one cached build.
        revision TEXT NOT NULL,
        title TEXT NOT NULL,
        body TEXT,
        created_at INTEGER NOT NULL,
        file_count INTEGER NOT NULL,
        byte_size INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS snapshots_project_idx
        ON snapshots(project_id, created_at DESC);
    CREATE INDEX IF NOT EXISTS snapshots_revision_idx
        ON snapshots(project_id, revision);

    CREATE TABLE IF NOT EXISTS snapshot_files (
        snapshot_id INTEGER NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        object TEXT NOT NULL,
        byte_size INTEGER NOT NULL,
        PRIMARY KEY (snapshot_id, path)
    );
    CREATE INDEX IF NOT EXISTS snapshot_files_object_idx ON snapshot_files(object);

    CREATE TABLE IF NOT EXISTS build_states (
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        source_ref TEXT NOT NULL,
        status TEXT NOT NULL,
        started_at INTEGER,
        finished_at INTEGER,
        duration_ms INTEGER,
        error_summary TEXT,
        diagnostics TEXT NOT NULL DEFAULT '[]',
        PRIMARY KEY (project_id, source_ref)
    );
";

/// Bump only when an existing table changes shape. Adding a table or an index
/// needs no bump, because `CREATE TABLE IF NOT EXISTS` adds it to a database
/// that predates it.
///
/// There is no migration path by design: Press has one user, and a stale
/// database is cheaper to delete than to migrate. The version exists so that a
/// mismatch says so plainly instead of failing later with a confusing SQL error.
const SCHEMA_VERSION: i32 = 3;

fn initialize(connection: &Connection) -> AppResult<()> {
    connection.execute_batch(SCHEMA)?;
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// Moves a database written by a different schema out of the way, returning
/// where it went. The write-ahead log and shared-memory files move with it: a
/// fresh database paired with someone else's `-wal` is worse than either alone.
fn retire_incompatible(path: &Path) -> AppResult<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let version: i32 = {
        let connection = Connection::open(path)?;
        connection.query_row("PRAGMA user_version", [], |row| row.get(0))?
    };
    if version == 0 || version == SCHEMA_VERSION {
        return Ok(None);
    }

    // Numbered rather than overwritten, so retiring twice does not destroy the
    // first one.
    let mut retired = path.with_extension(format!("schema{version}.old"));
    let mut attempt = 2;
    while retired.exists() {
        retired = path.with_extension(format!("schema{version}.{attempt}.old"));
        attempt += 1;
    }
    std::fs::rename(path, &retired)?;

    for suffix in ["-wal", "-shm"] {
        let companion = companion_path(path, suffix);
        if companion.is_file() {
            let _ = std::fs::rename(&companion, companion_path(&retired, suffix));
        }
    }
    eprintln!(
        "Press: database schema {version} is not schema {SCHEMA_VERSION}; set aside as {}",
        retired.display()
    );
    Ok(Some(retired))
}

/// SQLite's sidecars are the database path with a suffix appended to the whole
/// name, not a replaced extension.
fn companion_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BuildStatus;

    #[test]
    fn reopening_keeps_what_was_stored() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("press.db");
        let root = project_fixture(directory.path(), "thesis");
        {
            let database = Repository::open(&database_path).unwrap();
            add(&database, &root);
        }
        let reopened = Repository::open(&database_path).unwrap();
        assert_eq!(reopened.list_projects().unwrap().len(), 1);
    }

    /// Press has to start. Refusing to open meant a schema change crashed the
    /// application before it could explain itself.
    #[test]
    fn a_database_from_another_schema_is_set_aside_not_refused() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("press.db");
        let root = project_fixture(directory.path(), "thesis");
        {
            let stale = Repository::open(&database_path).unwrap();
            add(&stale, &root);
            let connection = Connection::open(&database_path).unwrap();
            connection
                .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        let reopened = Repository::open(&database_path).unwrap();
        assert!(reopened.list_projects().unwrap().is_empty(), "started fresh");

        let notice = reopened.notice().expect("the user is told what happened");
        assert!(notice.contains("set aside"), "{notice}");

        // The old database is kept, and still holds what it held.
        let retired = directory
            .path()
            .join(format!("press.schema{}.old", SCHEMA_VERSION + 1));
        assert!(retired.is_file(), "the previous database is preserved");
        let survivors: i64 = Connection::open(&retired)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(survivors, 1, "the retired database still has its projects");
    }

    #[test]
    fn sqlite_sidecars_travel_with_the_database() {
        // Appended to the whole name rather than replacing the extension, which
        // is how SQLite names them.
        assert_eq!(
            companion_path(Path::new("/data/press.sqlite3"), "-wal"),
            Path::new("/data/press.sqlite3-wal")
        );
        assert_eq!(
            companion_path(Path::new("/data/press.schema1.old"), "-shm"),
            Path::new("/data/press.schema1.old-shm")
        );
    }

    #[test]
    fn retiring_twice_does_not_destroy_the_first_one() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("press.db");
        let stale_version = SCHEMA_VERSION + 1;

        for _ in 0..2 {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .pragma_update(None, "user_version", stale_version)
                .unwrap();
            drop(connection);
            Repository::open(&database_path).unwrap();
        }

        assert!(
            directory
                .path()
                .join(format!("press.schema{stale_version}.old"))
                .is_file()
        );
        assert!(
            directory
                .path()
                .join(format!("press.schema{stale_version}.2.old"))
                .is_file()
        );
    }

    fn project_fixture(directory: &Path, name: &str) -> PathBuf {
        let root = directory.join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.tex"), "\\documentclass{article}").unwrap();
        root
    }

    fn add(repository: &Repository, root: &Path) -> Project {
        repository
            .upsert_project(NewProject {
                name: "Thesis",
                root_path: root.to_str().unwrap(),
                main_file: "main.tex",
                working_directory: ".",
                kind: DocumentKind::Latex,
                engine: Engine::PdfLatex,
            })
            .unwrap()
    }

    #[test]
    fn re_adding_the_same_document_updates_it_rather_than_duplicating() {
        let directory = tempfile::tempdir().unwrap();
        let database = Repository::open(&directory.path().join("press.db")).unwrap();
        let root = project_fixture(directory.path(), "thesis");

        let first = add(&database, &root);
        let again = database
            .upsert_project(NewProject {
                name: "Ignored",
                root_path: root.to_str().unwrap(),
                main_file: "main.tex",
                working_directory: ".",
                kind: DocumentKind::Latex,
                engine: Engine::XeLatex,
            })
            .unwrap();

        assert_eq!(first.id, again.id);
        assert_eq!(database.list_projects().unwrap().len(), 1);
        assert_eq!(again.engine, Engine::XeLatex, "settings are updated");
        // Re-adding must not overwrite a name the user chose.
        assert_eq!(again.name, "Thesis");
    }

    /// A project is a document, not a folder. Two essays in one directory are
    /// two projects; keying on the folder made the second overwrite the first.
    #[test]
    fn documents_sharing_a_folder_are_separate_projects() {
        let directory = tempfile::tempdir().unwrap();
        let database = Repository::open(&directory.path().join("press.db")).unwrap();
        let root = project_fixture(directory.path(), "writing");
        std::fs::write(root.join("essay.md"), "# An essay").unwrap();
        std::fs::write(root.join("talk.md"), "# A talk").unwrap();

        let essay = database
            .upsert_project(NewProject {
                name: "essay",
                root_path: root.to_str().unwrap(),
                main_file: "essay.md",
                working_directory: ".",
                kind: DocumentKind::Markdown,
                engine: Engine::PdfLatex,
            })
            .unwrap();
        let talk = database
            .upsert_project(NewProject {
                name: "talk",
                root_path: root.to_str().unwrap(),
                main_file: "talk.md",
                working_directory: ".",
                kind: DocumentKind::Markdown,
                engine: Engine::PdfLatex,
            })
            .unwrap();

        assert_ne!(essay.id, talk.id);
        assert_eq!(database.list_projects().unwrap().len(), 2);
        // Each keeps its own document and its own name.
        assert_eq!(database.get_project(essay.id).unwrap().main_file, "essay.md");
        assert_eq!(database.get_project(talk.id).unwrap().name, "talk");

        // Deleting one leaves the other alone.
        database.delete_project(essay.id).unwrap();
        assert_eq!(database.list_projects().unwrap().len(), 1);
        assert!(database.get_project(talk.id).is_ok());
    }

    #[test]
    fn artifacts_are_cached_per_version_and_engine() {
        let directory = tempfile::tempdir().unwrap();
        let database = Repository::open(&directory.path().join("press.db")).unwrap();
        let root = project_fixture(directory.path(), "thesis");
        let project = add(&database, &root);
        let snapshot = SourceRef::Snapshot("abc123".into());

        let first = directory.path().join("build-1.pdf");
        let second = directory.path().join("build-2.pdf");
        std::fs::write(&first, b"%PDF-1.7").unwrap();
        std::fs::write(&second, b"%PDF-1.7").unwrap();

        let (worktree_artifact, superseded) = database
            .record_artifact(NewArtifact {
                project_id: project.id,
                source_ref: &SourceRef::Worktree,
                engine: Engine::PdfLatex,
                pdf_path: &first,
                page_count: Some(12),
                byte_size: 8,
            })
            .unwrap();
        assert_eq!(worktree_artifact.revision, 1);
        assert!(superseded.is_none());

        let (replaced, superseded) = database
            .record_artifact(NewArtifact {
                project_id: project.id,
                source_ref: &SourceRef::Worktree,
                engine: Engine::PdfLatex,
                pdf_path: &second,
                page_count: Some(13),
                byte_size: 8,
            })
            .unwrap();
        assert_eq!(replaced.id, worktree_artifact.id);
        assert_eq!(replaced.revision, 2);
        assert_eq!(superseded, Some(first));

        // A snapshot is a separate cache entry, not a replacement.
        let (snapshot_artifact, _) = database
            .record_artifact(NewArtifact {
                project_id: project.id,
                source_ref: &snapshot,
                engine: Engine::PdfLatex,
                pdf_path: &second,
                page_count: Some(9),
                byte_size: 8,
            })
            .unwrap();
        assert_ne!(snapshot_artifact.id, worktree_artifact.id);
        assert!(
            database
                .artifact_for(project.id, &snapshot, Engine::PdfLatex)
                .unwrap()
                .is_some()
        );
        assert!(
            database
                .artifact_for(project.id, &snapshot, Engine::XeLatex)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn changing_the_engine_discards_incomparable_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let database = Repository::open(&directory.path().join("press.db")).unwrap();
        let root = project_fixture(directory.path(), "thesis");
        let project = add(&database, &root);
        let pdf = directory.path().join("build-1.pdf");
        std::fs::write(&pdf, b"%PDF-1.7").unwrap();
        database
            .record_artifact(NewArtifact {
                project_id: project.id,
                source_ref: &SourceRef::Worktree,
                engine: Engine::PdfLatex,
                pdf_path: &pdf,
                page_count: Some(3),
                byte_size: 8,
            })
            .unwrap();

        let (updated, discarded) = database
            .update_project(
                project.id,
                ProjectEdit {
                    engine: Some(Engine::LuaLatex),
                    ..ProjectEdit::default()
                },
            )
            .unwrap();
        assert_eq!(updated.engine, Engine::LuaLatex);
        assert_eq!(discarded, vec![pdf]);
        assert!(database.managed_pdf_paths().unwrap().is_empty());
    }

    #[test]
    fn renaming_keeps_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let database = Repository::open(&directory.path().join("press.db")).unwrap();
        let root = project_fixture(directory.path(), "thesis");
        let project = add(&database, &root);
        let pdf = directory.path().join("build-1.pdf");
        std::fs::write(&pdf, b"%PDF-1.7").unwrap();
        database
            .record_artifact(NewArtifact {
                project_id: project.id,
                source_ref: &SourceRef::Worktree,
                engine: Engine::PdfLatex,
                pdf_path: &pdf,
                page_count: None,
                byte_size: 8,
            })
            .unwrap();

        let (updated, discarded) = database
            .update_project(
                project.id,
                ProjectEdit {
                    name: Some("Dissertation".into()),
                    ..ProjectEdit::default()
                },
            )
            .unwrap();
        assert_eq!(updated.name, "Dissertation");
        assert!(discarded.is_empty());
        assert_eq!(database.managed_pdf_paths().unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_project_reports_its_orphaned_pdfs() {
        let directory = tempfile::tempdir().unwrap();
        let database = Repository::open(&directory.path().join("press.db")).unwrap();
        let root = project_fixture(directory.path(), "thesis");
        let project = add(&database, &root);
        let pdf = directory.path().join("build-1.pdf");
        std::fs::write(&pdf, b"%PDF-1.7").unwrap();
        database
            .record_artifact(NewArtifact {
                project_id: project.id,
                source_ref: &SourceRef::Worktree,
                engine: Engine::PdfLatex,
                pdf_path: &pdf,
                page_count: None,
                byte_size: 8,
            })
            .unwrap();

        let orphaned = database.delete_project(project.id).unwrap();
        assert_eq!(orphaned, vec![pdf]);
        assert!(database.list_projects().unwrap().is_empty());
        assert!(database.managed_pdf_paths().unwrap().is_empty());
        assert!(database.delete_project(project.id).is_err());
    }

    #[test]
    fn build_states_are_stored_per_source_reference() {
        let directory = tempfile::tempdir().unwrap();
        let database = Repository::open(&directory.path().join("press.db")).unwrap();
        let root = project_fixture(directory.path(), "thesis");
        let project = add(&database, &root);
        let snapshot = SourceRef::Snapshot("abc123".into());

        assert_eq!(
            database
                .build_state(project.id, &SourceRef::Worktree)
                .unwrap()
                .status,
            BuildStatus::Never
        );

        database
            .set_build_state(
                project.id,
                &BuildState {
                    source_ref: SourceRef::Worktree,
                    status: BuildStatus::Running,
                    started_at: Some(10),
                    finished_at: None,
                    duration_ms: None,
                    error_summary: None,
                    diagnostics: Vec::new(),
                },
            )
            .unwrap();
        database
            .set_build_state(
                project.id,
                &BuildState {
                    source_ref: snapshot.clone(),
                    status: BuildStatus::Error,
                    started_at: Some(10),
                    finished_at: Some(12),
                    duration_ms: Some(2000),
                    error_summary: Some("main.tex:4: Undefined control sequence.".into()),
                    diagnostics: vec![Diagnostic {
                        file: Some("main.tex".into()),
                        line: Some(4),
                        severity: crate::model::Severity::Error,
                        message: "Undefined control sequence.".into(),
                    }],
                },
            )
            .unwrap();

        let stored = database.build_state(project.id, &snapshot).unwrap();
        assert_eq!(stored.status, BuildStatus::Error);
        assert_eq!(stored.diagnostics.len(), 1);
        assert_eq!(stored.diagnostics[0].line, Some(4));
        assert_eq!(
            database
                .build_state(project.id, &SourceRef::Worktree)
                .unwrap()
                .status,
            BuildStatus::Running
        );
    }

    #[test]
    fn reopening_marks_unfinished_builds_as_interrupted() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("press.db");
        let root = project_fixture(directory.path(), "thesis");
        let project_id = {
            let database = Repository::open(&database_path).unwrap();
            let project = add(&database, &root);
            database
                .set_build_state(
                    project.id,
                    &BuildState {
                        source_ref: SourceRef::Worktree,
                        status: BuildStatus::Running,
                        started_at: Some(10),
                        finished_at: None,
                        duration_ms: None,
                        error_summary: None,
                        diagnostics: Vec::new(),
                    },
                )
                .unwrap();
            project.id
        };

        let reopened = Repository::open(&database_path).unwrap();
        let state = reopened.build_state(project_id, &SourceRef::Worktree).unwrap();
        assert_eq!(state.status, BuildStatus::Interrupted);
        assert!(state.error_summary.unwrap().contains("Press closed"));
    }



}
