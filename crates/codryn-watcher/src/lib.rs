use anyhow::Result;
use codryn_pipeline::{IndexMode, Pipeline};
use codryn_store::Store;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct Watcher {
    db_path: PathBuf,
    running: Arc<AtomicBool>,
}

impl Watcher {
    pub fn new(db_path: &Path) -> Self {
        Self {
            db_path: db_path.to_owned(),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Return all indexed project root paths from the store.
    fn project_roots(&self) -> Vec<PathBuf> {
        let db_file = self.db_path.join("graph.db");
        Store::open(&db_file)
            .ok()
            .and_then(|s| s.list_projects().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|p| PathBuf::from(p.root_path))
            .filter(|p| p.exists())
            .collect()
    }

    /// Run the watcher loop. Blocks until stop() is called.
    pub fn run(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);

        let roots = self.project_roots();
        if roots.is_empty() {
            tracing::info!("watcher: no indexed projects found, watcher idle");
            while self.running.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(5));
                if !self.project_roots().is_empty() {
                    tracing::info!("watcher: projects detected, restarting watcher");
                    break; // exit idle loop and fall through to watch setup
                }
            }
            if !self.running.load(Ordering::SeqCst) {
                return Ok(());
            }
        }

        tracing::info!(roots = ?roots, "watcher: watching {} project(s)", roots.len());

        let db = self.db_path.clone();
        let running = self.running.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = new_debouncer(Duration::from_secs(2), tx)?;

        for root in &roots {
            if let Err(e) = debouncer
                .watcher()
                .watch(root, notify::RecursiveMode::Recursive)
            {
                tracing::warn!(path = %root.display(), error = %e, "watcher: failed to watch path");
            }
        }

        while running.load(Ordering::SeqCst) {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Ok(events)) => {
                    // Group changed events by project root
                    let changed_roots: std::collections::HashSet<PathBuf> = events
                        .iter()
                        .filter(|e| e.kind == DebouncedEventKind::Any)
                        .filter_map(|e| roots.iter().find(|r| e.path.starts_with(r)).cloned())
                        .collect();

                    for root in changed_roots {
                        tracing::info!(path = %root.display(), "watcher: changes detected, re-indexing");
                        let pipeline = Pipeline::new(&root, &db, IndexMode::Fast);
                        if let Err(e) = pipeline.run() {
                            tracing::error!(error = %e, "watcher: re-index failed");
                        }
                    }
                }
                Ok(Err(e)) => tracing::warn!(error = %e, "watcher: notify error"),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }

        tracing::info!("watcher: stopped");
        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Project, Store};

    #[test]
    fn test_project_roots_empty_when_no_projects() {
        let dir = tempfile::tempdir().unwrap();
        let w = Watcher::new(dir.path());
        // No graph.db yet — should return empty
        assert!(w.project_roots().is_empty());
    }

    #[test]
    fn test_project_roots_returns_existing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("graph.db");
        let s = Store::open(&db).unwrap();
        let root = dir.path().to_str().unwrap().to_owned();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: root.clone(),
        })
        .unwrap();
        drop(s);

        let w = Watcher::new(dir.path());
        let roots = w.project_roots();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0], PathBuf::from(&root));
    }

    #[test]
    fn test_project_roots_filters_nonexistent_paths() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("graph.db");
        let s = Store::open(&db).unwrap();
        s.upsert_project(&Project {
            name: "ghost".into(),
            indexed_at: "now".into(),
            root_path: "/nonexistent/path/xyz".into(),
        })
        .unwrap();
        drop(s);

        let w = Watcher::new(dir.path());
        // Non-existent path should be filtered out
        assert!(w.project_roots().is_empty());
    }
}
