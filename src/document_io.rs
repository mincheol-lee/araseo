//! File operations and refresh probes performed by workers.
use crate::document::{DiskRevision, Document};
use crate::tabs::TabId;
use std::path::PathBuf;
#[derive(Clone, Copy)]
pub enum Operation {
    Save(bool),
    Reload,
}
impl Operation {
    /// Reject reloads invalidated by typing while an earlier operation ran.
    pub fn prepare(self, document: &Document, requested_version: u64) -> Option<Document> {
        if matches!(self, Self::Reload) && document.edit_version() != requested_version {
            return None;
        }
        Some(document.save_snapshot())
    }
}
pub struct Outcome {
    pub id: TabId,
    pub version: u64,
    pub host: PathBuf,
    pub operation: Operation,
    pub result: Result<Document, String>,
    pub conflict: bool,
}
pub fn run(id: TabId, mut document: Document, operation: Operation) -> Outcome {
    let version = document.edit_version();
    let host = document.host_path.clone();
    let result = match operation {
        Operation::Save(overwrite) => document.save(overwrite).map(|_| None),
        Operation::Reload => Document::open(document.linux_path.clone(), host.clone()).map(Some),
    };
    let conflict = result.is_err() && document.changed_on_disk();
    Outcome {
        id,
        version,
        host,
        operation,
        result: result
            .map(|loaded| loaded.unwrap_or(document))
            .map_err(|e| e.to_string()),
        conflict,
    }
}
pub struct Probe {
    pub id: TabId,
    pub version: u64,
    pub path: PathBuf,
    pub host: PathBuf,
    pub revision: DiskRevision,
    pub dirty: bool,
}
impl Probe {
    pub fn new(id: TabId, document: &Document) -> Self {
        Self {
            id,
            version: document.edit_version(),
            path: document.linux_path.clone(),
            host: document.host_path.clone(),
            revision: document.disk_revision.clone(),
            dirty: document.dirty,
        }
    }
    pub fn is_current(&self, document: &Document) -> bool {
        document.host_path == self.host
            && document.edit_version() == self.version
            && document.disk_revision == self.revision
    }
    pub fn check(self) -> (Self, Result<Option<Document>, String>) {
        let result = (|| {
            let metadata = std::fs::metadata(&self.host).map_err(|e| e.to_string())?;
            let revision = DiskRevision {
                len: metadata.len(),
                modified: metadata.modified().ok(),
            };
            if revision == self.revision {
                return Ok(None);
            }
            if self.dirty {
                return Err("File changed outside Araseo".into());
            }
            Document::open(self.path.clone(), self.host.clone())
                .map(Some)
                .map_err(|e| e.to_string())
        })();
        (self, result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_completion_keeps_later_edits_and_undo_history() {
        let path = std::env::temp_dir().join(format!("araseo-async-save-{}", std::process::id()));
        std::fs::write(&path, "original").unwrap();
        let mut live = Document::open(path.clone(), path.clone()).unwrap();
        live.set_text("saved edit".into());
        let snapshot = live.save_snapshot();
        live.set_text("typed during save".into());
        let result = run(1, snapshot, Operation::Save(false));
        live.accept_saved(&result.result.unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved edit");
        assert_eq!(live.text, "typed during save");
        assert!(live.dirty);
        live.undo();
        assert_eq!(live.text, "saved edit");
        assert!(!live.dirty);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    #[test]
    fn stale_refresh_cannot_replace_new_edits_or_a_newly_reloaded_buffer() {
        let path = std::env::temp_dir().join(format!("araseo-refresh-{}", std::process::id()));
        std::fs::write(&path, "first").unwrap();
        let mut live = Document::open(path.clone(), path.clone()).unwrap();
        let probe = Probe::new(3, &live);
        live.set_text("typing".into());
        assert!(!probe.is_current(&live));
        let version = live.edit_version();
        live.accept_reloaded(Document::open(path.clone(), path.clone()).unwrap());
        assert!(live.edit_version() > version);
        assert!(!probe.is_current(&live));
        let probe = Probe::new(3, &live);
        std::fs::write(&path, "external change").unwrap();
        let (probe, result) = probe.check();
        assert!(probe.is_current(&live));
        live.accept_reloaded(result.unwrap().unwrap());
        assert_eq!(live.text, "external change");
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    #[test]
    fn queued_reload_preserves_edits_made_before_its_worker_starts() {
        let path =
            std::env::temp_dir().join(format!("araseo-queued-reload-{}", std::process::id()));
        std::fs::write(&path, "disk").unwrap();
        let mut live = Document::open(path.clone(), path.clone()).unwrap();
        let requested = live.edit_version();
        assert!(Operation::Reload.prepare(&live, requested).is_some());
        live.set_text("typed while waiting for another save".into());
        assert!(Operation::Reload.prepare(&live, requested).is_none());
        let snapshot = Operation::Save(false).prepare(&live, requested).unwrap();
        assert_eq!(snapshot.text, live.text);
        assert!(live.dirty);
        std::fs::remove_file(path).unwrap();
    }
}
