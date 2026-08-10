//! PDFs Press is only showing.
//!
//! A project is a source document: everything Press keeps about one — its
//! history, its builds, its engine — follows from the document it compiles. A
//! PDF handed to Press on the command line has none of that, so it is not a
//! project and never reaches the library. It is a guest: opened, watched while
//! it is on screen, and forgotten on the way out.
//!
//! Watched, because that is the whole point of viewing a PDF Press did not
//! build. Something else is rebuilding it — a Makefile, `latexmk -pvc`, a
//! colleague's script — and a viewer that does not notice is a viewer you have
//! to keep reopening.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::{
    error::{AppError, AppResult},
    model::LooseDocument,
};

/// Long enough for a build to finish writing the file, short enough not to be
/// noticed. A PDF appears in pieces while latexmk is writing it.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// Every PDF Press is showing but does not own.
///
/// Ids count down from -1. Artifact ids come from SQLite and count up from 1,
/// so the two can share one space without ever colliding — which is what lets
/// the page protocol, the layout call and the search work on a loose PDF with
/// no changes at all.
#[derive(Default)]
pub struct Viewing {
    open: Mutex<Registry>,
}

#[derive(Default)]
struct Registry {
    next: i64,
    documents: HashMap<i64, Entry>,
}

struct Entry {
    path: PathBuf,
    /// Bumped when the file changes, which is what makes the viewer redraw.
    revision: i64,
    /// Dropped with the entry, which stops the watcher and ends its task.
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Viewing {
    /// Takes a PDF for viewing and starts watching it.
    ///
    /// Press shows one document at a time, so this replaces whatever was open:
    /// nothing is left watching a file nobody is looking at.
    pub fn open(self: &Arc<Self>, app: &AppHandle, path: &Path) -> AppResult<LooseDocument> {
        let document = self.register(path)?;
        let watcher = self
            .clone()
            .watch(app.clone(), document.id, Path::new(&document.path));
        if let Ok(mut registry) = self.lock()
            && let Some(entry) = registry.documents.get_mut(&document.id)
        {
            entry._watcher = watcher;
        }
        Ok(document)
    }

    /// Takes the document without watching it, which is the whole of what makes
    /// an id resolve to a file.
    fn register(&self, path: &Path) -> AppResult<LooseDocument> {
        let canonical = path.canonicalize().map_err(|error| {
            AppError::NotFound(format!("{} cannot be opened: {error}", path.display()))
        })?;
        if !canonical.is_file() {
            return Err(AppError::InvalidInput(format!(
                "{} is not a file",
                canonical.display()
            )));
        }
        // Read once here rather than failing later inside the viewer, where the
        // only thing left to say would be that a page did not render.
        let document = crate::render::open(&canonical)?;
        crate::render::page_count(&document)?;
        drop(document);

        let name = canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| canonical.to_string_lossy().into_owned());

        let mut registry = self.lock()?;
        // One document at a time, so nothing is left watching a file that is no
        // longer on screen.
        registry.documents.clear();
        registry.next -= 1;
        let id = registry.next;
        registry.documents.insert(
            id,
            Entry {
                path: canonical.clone(),
                revision: 1,
                _watcher: None,
            },
        );

        Ok(LooseDocument {
            id,
            name,
            path: canonical.to_string_lossy().into_owned(),
            revision: 1,
        })
    }

    pub fn close(&self, id: i64) -> AppResult<()> {
        self.lock()?.documents.remove(&id);
        Ok(())
    }

    /// Where a loose id points, or `None` for anything else — including an
    /// artifact id, which belongs to the database.
    pub fn path(&self, id: i64) -> Option<PathBuf> {
        self.open
            .lock()
            .ok()
            .and_then(|registry| registry.documents.get(&id).map(|entry| entry.path.clone()))
    }

    fn lock(&self) -> AppResult<std::sync::MutexGuard<'_, Registry>> {
        self.open
            .lock()
            .map_err(|_| AppError::Task("the open document registry was poisoned".into()))
    }

    /// Watches the file's directory rather than the file: a build replaces a
    /// PDF by writing a new one and renaming it over the old, which drops a
    /// watch held on the file itself.
    fn watch(
        self: Arc<Self>,
        app: AppHandle,
        id: i64,
        file: &Path,
    ) -> Option<notify::RecommendedWatcher> {
        let directory = file.parent()?.to_path_buf();
        let target = file.to_path_buf();
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else { return };
            // Opening the file to draw it is not a change to it.
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            if event.paths.iter().any(|path| path == &target) {
                let _ = sender.send(());
            }
        })
        .ok()?;
        watcher.watch(&directory, RecursiveMode::NonRecursive).ok()?;
        tauri::async_runtime::spawn(reload_loop(self, app, id, receiver));
        Some(watcher)
    }

    /// Records that the file changed and returns what to tell the viewer.
    fn touch(&self, id: i64) -> Option<i64> {
        let mut registry = self.lock().ok()?;
        let entry = registry.documents.get_mut(&id)?;
        entry.revision += 1;
        Some(entry.revision)
    }
}

/// Coalesces a burst of writes into one reload, then tells the viewer to draw
/// the file again.
async fn reload_loop(
    viewing: Arc<Viewing>,
    app: AppHandle,
    id: i64,
    mut events: mpsc::UnboundedReceiver<()>,
) {
    while events.recv().await.is_some() {
        loop {
            let settle = tokio::time::sleep(SETTLE);
            tokio::pin!(settle);
            tokio::select! {
                () = &mut settle => break,
                message = events.recv() => if message.is_none() { return },
            }
        }
        // Gone with the entry: the document was closed while this was waiting.
        let Some(revision) = viewing.touch(id) else {
            return;
        };
        let _ = app.emit(
            "viewing-changed",
            serde_json::json!({ "id": id, "revision": revision }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF, written by hand so the test needs no TeX.
    fn pdf_at(path: &Path) {
        let body = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj\n\
trailer<</Root 1 0 R>>\n";
        std::fs::write(path, body).unwrap();
    }

    /// The whole point of the id being negative: an artifact's never is, so one
    /// space serves both and the page protocol needs no second route.
    #[test]
    fn a_viewed_pdf_takes_an_id_no_artifact_can_have() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("paper.pdf");
        pdf_at(&file);

        let viewing = Viewing::default();
        let first = viewing.register(&file).unwrap();
        assert!(first.id < 0, "ids count down from -1");
        assert_eq!(first.name, "paper.pdf");
        assert_eq!(first.revision, 1);
        assert_eq!(viewing.path(first.id).as_deref(), Some(file.canonicalize().unwrap().as_path()));

        // Opening another replaces it: Press shows one document at a time, and
        // a registry that grew would keep watching files nobody is reading.
        let second = viewing.register(&file).unwrap();
        assert_ne!(second.id, first.id);
        assert_eq!(viewing.path(first.id), None, "the first one is let go");
        assert!(viewing.path(second.id).is_some());

        viewing.close(second.id).unwrap();
        assert_eq!(viewing.path(second.id), None);
    }

    /// Nothing resolves an artifact id here. The database owns those, and a
    /// mix-up would serve a file from the wrong side of the storage check.
    #[test]
    fn an_artifact_id_is_not_a_viewed_document() {
        let viewing = Viewing::default();
        assert_eq!(viewing.path(1), None);
        assert_eq!(viewing.path(0), None);
        assert_eq!(viewing.path(-1), None, "not until something is opened");
    }

    #[test]
    fn a_file_that_is_not_a_pdf_is_refused_at_the_door() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("notes.pdf");
        std::fs::write(&file, b"this is not a PDF").unwrap();
        assert!(Viewing::default().register(&file).is_err());
        assert!(
            Viewing::default()
                .register(&directory.path().join("nothing.pdf"))
                .is_err()
        );
    }

    /// A rebuild is what bumps the revision, and the revision is what makes the
    /// viewer draw the file again.
    #[test]
    fn a_change_moves_the_revision_on() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("paper.pdf");
        pdf_at(&file);

        let viewing = Viewing::default();
        let document = viewing.register(&file).unwrap();
        assert_eq!(viewing.touch(document.id), Some(2));
        assert_eq!(viewing.touch(document.id), Some(3));
        assert_eq!(viewing.touch(-999), None, "nothing is open under that id");
    }
}
