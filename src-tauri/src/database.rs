use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    error::{AppError, AppResult},
    model::ProjectSummary,
};

pub struct Repository {
    connection: Mutex<Connection>,
}

pub struct NewProject<'a> {
    pub name: &'a str,
    pub root_path: &'a str,
    pub main_file: &'a str,
    pub working_directory: &'a str,
    pub engine: &'a str,
}

impl Repository {
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                root_path TEXT NOT NULL UNIQUE,
                main_file TEXT NOT NULL,
                working_directory TEXT NOT NULL,
                engine TEXT NOT NULL DEFAULT 'pdflatex',
                created_at INTEGER NOT NULL,
                last_opened_at INTEGER NOT NULL,
                build_status TEXT NOT NULL DEFAULT 'never',
                last_build_at INTEGER,
                last_build_duration_ms INTEGER,
                last_error TEXT,
                last_pdf_path TEXT,
                artifact_revision INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS projects_last_opened_idx
                ON projects(last_opened_at DESC);
            UPDATE projects
                SET build_status = 'interrupted',
                    last_error = 'The previous build ended when Press closed.'
                WHERE build_status = 'building';
            PRAGMA user_version = 1;
            ",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn list_projects(&self) -> AppResult<Vec<ProjectSummary>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, name, root_path, main_file, working_directory, engine,
                    build_status, last_build_at, last_build_duration_ms,
                    last_error, last_pdf_path, artifact_revision
             FROM projects
             ORDER BY last_opened_at DESC, name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], map_project)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_project(&self, id: i64) -> AppResult<ProjectSummary> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT id, name, root_path, main_file, working_directory, engine,
                        build_status, last_build_at, last_build_duration_ms,
                        last_error, last_pdf_path, artifact_revision
                 FROM projects WHERE id = ?1",
                [id],
                map_project,
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound(format!("project {id} does not exist")))
    }

    pub fn add_or_update_project(&self, project: NewProject<'_>) -> AppResult<ProjectSummary> {
        let now = unix_timestamp();
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO projects (
                name, root_path, main_file, working_directory, engine,
                created_at, last_opened_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(root_path) DO UPDATE SET
                name = excluded.name,
                main_file = excluded.main_file,
                working_directory = excluded.working_directory,
                engine = excluded.engine,
                last_opened_at = excluded.last_opened_at",
            params![
                project.name,
                project.root_path,
                project.main_file,
                project.working_directory,
                project.engine,
                now
            ],
        )?;
        let id: i64 = connection.query_row(
            "SELECT id FROM projects WHERE root_path = ?1",
            [project.root_path],
            |row| row.get(0),
        )?;
        drop(connection);
        self.get_project(id)
    }

    pub fn touch_project(&self, id: i64) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE projects SET last_opened_at = ?2 WHERE id = ?1",
            params![id, unix_timestamp()],
        )?;
        Ok(())
    }

    pub fn record_build_started(&self, id: i64) -> AppResult<ProjectSummary> {
        self.lock()?.execute(
            "UPDATE projects SET build_status = 'building', last_error = NULL WHERE id = ?1",
            [id],
        )?;
        self.get_project(id)
    }

    pub fn record_build_success(
        &self,
        id: i64,
        duration_ms: i64,
        pdf_path: &Path,
    ) -> AppResult<ProjectSummary> {
        let pdf_path = pdf_path
            .to_str()
            .ok_or_else(|| AppError::InvalidInput("cached PDF path is not valid UTF-8".into()))?;
        self.lock()?.execute(
            "UPDATE projects SET
                build_status = 'success',
                last_build_at = ?2,
                last_build_duration_ms = ?3,
                last_error = NULL,
                last_pdf_path = ?4,
                artifact_revision = artifact_revision + 1
             WHERE id = ?1",
            params![id, unix_timestamp(), duration_ms, pdf_path],
        )?;
        self.get_project(id)
    }

    pub fn record_build_failure(
        &self,
        id: i64,
        duration_ms: i64,
        error: &str,
    ) -> AppResult<ProjectSummary> {
        self.lock()?.execute(
            "UPDATE projects SET
                build_status = 'error',
                last_build_at = ?2,
                last_build_duration_ms = ?3,
                last_error = ?4
             WHERE id = ?1",
            params![id, unix_timestamp(), duration_ms, error],
        )?;
        self.get_project(id)
    }

    pub fn pdf_path(&self, id: i64) -> AppResult<PathBuf> {
        let connection = self.lock()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT last_pdf_path FROM projects WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        value
            .map(PathBuf::from)
            .ok_or_else(|| AppError::NotFound("this project has no successful PDF yet".into()))
    }

    pub fn managed_pdf_paths(&self) -> AppResult<Vec<PathBuf>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT last_pdf_path FROM projects WHERE last_pdf_path IS NOT NULL")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| row.map(PathBuf::from))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn lock(&self) -> AppResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| AppError::Task("database lock was poisoned".into()))
    }
}

fn map_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectSummary> {
    let root_path: String = row.get(2)?;
    let pdf_path: Option<String> = row.get(10)?;
    let has_pdf = pdf_path
        .as_deref()
        .is_some_and(|path| Path::new(path).is_file());
    Ok(ProjectSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        root_path: root_path.clone(),
        main_file: row.get(3)?,
        working_directory: row.get(4)?,
        engine: row.get(5)?,
        build_status: row.get(6)?,
        last_build_at: row.get(7)?,
        last_build_duration_ms: row.get(8)?,
        last_error: row.get(9)?,
        artifact_revision: row.get(11)?,
        has_pdf,
        path_available: Path::new(&root_path).is_dir(),
    })
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_updates_projects_without_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let database = Repository::open(&directory.path().join("press.db")).unwrap();
        let root = directory.path().join("thesis");
        std::fs::create_dir(&root).unwrap();
        let root = root.to_str().unwrap();

        let first = database
            .add_or_update_project(NewProject {
                name: "Thesis",
                root_path: root,
                main_file: "main.tex",
                working_directory: ".",
                engine: "pdflatex",
            })
            .unwrap();
        let second = database
            .add_or_update_project(NewProject {
                name: "Renamed",
                root_path: root,
                main_file: "book.tex",
                working_directory: ".",
                engine: "xelatex",
            })
            .unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(database.list_projects().unwrap().len(), 1);
        assert_eq!(second.main_file, "book.tex");
        assert_eq!(second.engine, "xelatex");
    }

    #[test]
    fn marks_an_abandoned_build_as_interrupted_on_startup() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("press.db");
        let root = directory.path().join("thesis");
        std::fs::create_dir(&root).unwrap();

        let project_id = {
            let database = Repository::open(&database_path).unwrap();
            let project = database
                .add_or_update_project(NewProject {
                    name: "Thesis",
                    root_path: root.to_str().unwrap(),
                    main_file: "main.tex",
                    working_directory: ".",
                    engine: "pdflatex",
                })
                .unwrap();
            database.record_build_started(project.id).unwrap();
            project.id
        };

        let reopened = Repository::open(&database_path).unwrap();
        let project = reopened.get_project(project_id).unwrap();
        assert_eq!(project.build_status, "interrupted");
        assert!(project.last_error.unwrap().contains("previous build"));
    }
}
