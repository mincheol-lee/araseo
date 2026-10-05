//! Persist the visible workspace between normal application runs.

use crate::document::{DiskRevision, Document};
use crate::tabs::{TabGroups, TabId};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const FORMAT: u32 = 1;
const MAX_SESSION_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    version: u32,
    distro: String,
    root: PathBuf,
    pub tabs: Vec<SavedTab>,
    pub groups: TabGroups,
    pub sidebar_width: f32,
    pub sidebar_view: i32,
    pub window_width: u32,
    pub window_height: u32,
    pub maximized: bool,
    pub maximized_group: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedTab {
    pub id: TabId,
    pub path: PathBuf,
    pub kind: SavedKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SavedKind {
    File {
        unsaved_text: Option<String>,
        revision: Option<SavedRevision>,
    },
    Preview,
    Terminal {
        number: u32,
    },
    Diff,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedRevision {
    len: u64,
    modified_nanos: Option<u128>,
}

impl SavedKind {
    pub fn file(document: &Document) -> Self {
        Self::File {
            unsaved_text: document.dirty.then(|| document.text.clone()),
            revision: document.dirty.then(|| SavedRevision {
                len: document.disk_revision.len,
                modified_nanos: document
                    .disk_revision
                    .modified
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos()),
            }),
        }
    }

    pub fn restore_file(&self, document: &mut Document) {
        let Self::File {
            unsaved_text: Some(text),
            revision,
        } = self
        else {
            return;
        };
        document.set_text(text.clone());
        if let Some(revision) = revision {
            document.disk_revision = DiskRevision {
                len: revision.len,
                modified: revision.modified_nanos.and_then(|nanos| {
                    let seconds = u64::try_from(nanos / 1_000_000_000).ok()?;
                    UNIX_EPOCH.checked_add(Duration::new(seconds, (nanos % 1_000_000_000) as u32))
                }),
            };
        }
    }
}

impl Session {
    pub fn new(distro: String, root: PathBuf, tabs: Vec<SavedTab>, groups: TabGroups) -> Self {
        Self {
            version: FORMAT,
            distro,
            root,
            tabs,
            groups,
            sidebar_width: 250.0,
            sidebar_view: 0,
            window_width: 1200,
            window_height: 800,
            maximized: false,
            maximized_group: None,
        }
    }

    pub fn load(path: &Path, distro: &str, root: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        if metadata.len() > MAX_SESSION_BYTES as u64 {
            return None;
        }
        let session: Self = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        if session.version != FORMAT
            || session.distro != distro
            || session.root != root
            || session.tabs.len() > 128
            || !session.sidebar_width.is_finite()
        {
            return None;
        }
        Some(session)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_SESSION_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "workspace session is too large",
            ));
        }
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }
}

pub fn settings_path(distro: &str, root: &Path) -> Option<PathBuf> {
    let history = crate::workspace_history::settings_path(distro, root)?;
    let name = history.file_stem()?.to_str()?;
    Some(
        history
            .parent()?
            .parent()?
            .join("sessions")
            .join(format!("{name}.json")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_session_round_trip_and_scope() {
        let path = std::env::temp_dir().join(format!("araseo-session-{}.json", std::process::id()));
        let mut groups = TabGroups::default();
        groups.add(7, 0);
        let session = Session::new(
            "Ubuntu".into(),
            "/projects/a".into(),
            vec![SavedTab {
                id: 7,
                path: "/projects/a/src/main.rs".into(),
                kind: SavedKind::File {
                    unsaved_text: None,
                    revision: None,
                },
            }],
            groups,
        );
        session.save(&path).unwrap();
        let restored = Session::load(&path, "Ubuntu", Path::new("/projects/a")).unwrap();
        assert_eq!(restored.tabs[0].id, 7);
        assert!(Session::load(&path, "Other", Path::new("/projects/a")).is_none());
        assert!(Session::load(&path, "Ubuntu", Path::new("/projects/b")).is_none());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn unsaved_text_restores_with_original_disk_revision() {
        let path =
            std::env::temp_dir().join(format!("araseo-session-document-{}", std::process::id()));
        fs::write(&path, "original").unwrap();
        let mut original = Document::open("/project/file.txt".into(), path.clone()).unwrap();
        original.set_text("unsaved edit".into());
        let kind = SavedKind::file(&original);
        let mut groups = TabGroups::default();
        groups.add(1, 0);
        let session_path = path.with_extension("session.json");
        Session::new(
            "Ubuntu".into(),
            "/project".into(),
            vec![SavedTab {
                id: 1,
                path: "/project/file.txt".into(),
                kind,
            }],
            groups,
        )
        .save(&session_path)
        .unwrap();
        fs::write(&path, "changed on disk after exit").unwrap();
        let mut reopened = Document::open("/project/file.txt".into(), path.clone()).unwrap();
        let restored = Session::load(&session_path, "Ubuntu", Path::new("/project")).unwrap();
        restored.tabs[0].kind.restore_file(&mut reopened);
        assert_eq!(reopened.text, "unsaved edit");
        assert!(reopened.dirty);
        assert!(reopened.changed_on_disk());
        fs::remove_file(path).unwrap();
        fs::remove_file(session_path).unwrap();
    }
}

/// Load the focused tab first, then other selected panes, then background tabs.
pub fn restore_order(
    mut tabs: Vec<SavedTab>,
    focused: Option<TabId>,
    selected: &std::collections::HashSet<TabId>,
) -> Vec<SavedTab> {
    tabs.sort_by_key(|tab| {
        if Some(tab.id) == focused {
            0
        } else if selected.contains(&tab.id) {
            1
        } else {
            2
        }
    });
    tabs
}
#[cfg(test)]
mod order_tests {
    use super::*;
    #[test]
    fn prioritizes_visible_tabs_and_keeps_background_order() {
        let tabs = (0..5)
            .map(|id| SavedTab {
                id,
                path: PathBuf::from("file"),
                kind: SavedKind::Preview,
            })
            .collect();
        assert_eq!(
            restore_order(tabs, Some(3), &[1, 3].into())
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            vec![3, 1, 0, 2, 4]
        );
    }
}
