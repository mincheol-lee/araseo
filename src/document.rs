use crate::workspace::MAX_EDITABLE_BYTES;
use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiskRevision {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub struct Document {
    pub linux_path: PathBuf,
    pub host_path: PathBuf,
    pub text: String,
    saved_text: String,
    pub line_ending: LineEnding,
    pub dirty: bool,
    pub disk_revision: DiskRevision,
    undo_stack: Vec<TextChange>,
    redo_stack: Vec<TextChange>,
}

#[derive(Clone, Debug)]
struct TextChange {
    start: usize,
    removed: String,
    inserted: String,
}

impl Document {
    pub fn open(linux_path: PathBuf, host_path: PathBuf) -> Result<Self> {
        let metadata = fs::metadata(&host_path)
            .with_context(|| format!("cannot read {}", linux_path.display()))?;
        if metadata.len() > MAX_EDITABLE_BYTES {
            bail!("file is larger than 2 MiB");
        }
        let bytes = fs::read(&host_path)?;
        if bytes.contains(&0) {
            bail!("binary files are not editable");
        }
        let raw = String::from_utf8(bytes).context("file is not valid UTF-8")?;
        let line_ending = if raw.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        let text = raw.replace("\r\n", "\n");
        Ok(Self {
            linux_path,
            host_path,
            saved_text: text.clone(),
            text,
            line_ending,
            dirty: false,
            disk_revision: revision_from(&metadata),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        })
    }

    pub fn set_text(&mut self, text: String) {
        if self.text != text {
            self.undo_stack.push(text_change(&self.text, &text));
            if self.undo_stack.len() > 1_000 {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
            self.text = text;
            self.dirty = self.text != self.saved_text;
        }
    }

    pub fn undo(&mut self) -> Option<usize> {
        let Some(change) = self.undo_stack.pop() else {
            return None;
        };
        let end = change.start + change.inserted.len();
        if end > self.text.len() || !self.text.is_char_boundary(change.start) || !self.text.is_char_boundary(end) {
            return None;
        }
        self.text.replace_range(change.start..end, &change.removed);
        self.dirty = self.text != self.saved_text;
        let cursor = change.start + change.removed.len();
        self.redo_stack.push(change);
        Some(cursor)
    }

    pub fn redo(&mut self) -> Option<usize> {
        let Some(change) = self.redo_stack.pop() else {
            return None;
        };
        let end = change.start + change.removed.len();
        if end > self.text.len() || !self.text.is_char_boundary(change.start) || !self.text.is_char_boundary(end) {
            return None;
        }
        self.text.replace_range(change.start..end, &change.inserted);
        self.dirty = self.text != self.saved_text;
        let cursor = change.start + change.inserted.len();
        self.undo_stack.push(change);
        Some(cursor)
    }

    pub fn changed_on_disk(&self) -> bool {
        fs::metadata(&self.host_path)
            .map(|metadata| revision_from(&metadata) != self.disk_revision)
            .unwrap_or(true)
    }

    pub fn save(&mut self, overwrite_external: bool) -> Result<()> {
        if self.changed_on_disk() && !overwrite_external {
            bail!("file changed on disk; refresh or explicitly overwrite it");
        }
        let output = match self.line_ending {
            LineEnding::Lf => self.text.clone(),
            LineEnding::CrLf => self.text.replace('\n', "\r\n"),
        };
        atomic_replace(&self.host_path, output.as_bytes())
            .with_context(|| format!("cannot save {}", self.linux_path.display()))?;
        let metadata = fs::metadata(&self.host_path)?;
        self.disk_revision = revision_from(&metadata);
        self.saved_text.clone_from(&self.text);
        self.dirty = false;
        Ok(())
    }

    pub fn title(&self) -> String {
        self.linux_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

fn text_change(before: &str, after: &str) -> TextChange {
    let mut start = 0;
    for ((before_index, before_char), (after_index, after_char)) in
        before.char_indices().zip(after.char_indices())
    {
        if before_char != after_char {
            break;
        }
        start = before_index + before_char.len_utf8();
        debug_assert_eq!(start, after_index + after_char.len_utf8());
    }

    let before_tail = &before[start..];
    let after_tail = &after[start..];
    let mut suffix = 0;
    for (before_char, after_char) in before_tail.chars().rev().zip(after_tail.chars().rev()) {
        if before_char != after_char {
            break;
        }
        suffix += before_char.len_utf8();
    }
    suffix = suffix.min(before_tail.len()).min(after_tail.len());

    TextChange {
        start,
        removed: before[start..before.len() - suffix].to_string(),
        inserted: after[start..after.len() - suffix].to_string(),
    }
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .context("file path has no name")?
        .to_string_lossy();
    let parent = path.parent().context("file path has no parent")?;
    let suffix = format!("{}.{}", std::process::id(), timestamp_suffix());
    let temporary = parent.join(format!(".{name}.araseo-{suffix}.tmp"));
    let backup = parent.join(format!(".{name}.araseo-{suffix}.bak"));
    let permissions = fs::metadata(path)?.permissions();

    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::set_permissions(&temporary, permissions)?;

        fs::rename(path, &backup)?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::rename(&backup, path);
            return Err(error.into());
        }
        let _ = fs::remove_file(&backup);
        Ok(())
    })();

    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn timestamp_suffix() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn revision_from(metadata: &fs::Metadata) -> DiskRevision {
    DiskRevision {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    }
}

pub fn line_numbers(text: &str) -> String {
    let count = text.bytes().filter(|byte| *byte == b'\n').count() + 1;
    (1..=count)
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_number_text_tracks_lines() {
        assert_eq!(line_numbers(""), "1");
        assert_eq!(line_numbers("a\nb\n"), "1\n2\n3");
    }

    #[test]
    fn preserves_crlf_and_detects_external_changes() {
        let directory =
            std::env::temp_dir().join(format!("araseo-document-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let path = directory.join("sample.txt");
        fs::write(&path, b"one\r\ntwo\r\n").unwrap();

        let mut document = Document::open(PathBuf::from("/sample.txt"), path.clone()).unwrap();
        assert_eq!(document.line_ending, LineEnding::CrLf);
        document.set_text("one\ntwo\nthree\n".into());
        document.save(false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one\r\ntwo\r\nthree\r\n");

        fs::write(&path, b"external change with a different length").unwrap();
        assert!(document.changed_on_disk());
        assert!(document.save(false).is_err());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn undoing_to_saved_text_clears_dirty_state() {
        let directory =
            std::env::temp_dir().join(format!("araseo-document-undo-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let path = directory.join("sample.txt");
        fs::write(&path, b"original").unwrap();

        let mut document = Document::open(PathBuf::from("/sample.txt"), path).unwrap();
        document.set_text("changed".into());
        assert!(document.dirty);
        assert!(document.undo().is_some());
        assert!(!document.dirty);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn undo_and_redo_preserve_unicode_changes() {
        let directory = std::env::temp_dir().join(format!(
            "araseo-document-unicode-undo-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let path = directory.join("sample.txt");
        fs::write(&path, "한글 원문").unwrap();

        let mut document = Document::open(PathBuf::from("/sample.txt"), path).unwrap();
        document.set_text("한글 수정문".into());
        assert!(document.undo().is_some());
        assert_eq!(document.text, "한글 원문");
        assert!(document.redo().is_some());
        assert_eq!(document.text, "한글 수정문");
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn separate_documents_never_share_undo_history() {
        let directory = std::env::temp_dir().join(format!(
            "araseo-document-tabs-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let first_path = directory.join("first.txt");
        let second_path = directory.join("second.txt");
        fs::write(&first_path, "first original").unwrap();
        fs::write(&second_path, "second original").unwrap();

        let mut first = Document::open(PathBuf::from("/first.txt"), first_path).unwrap();
        let mut second = Document::open(PathBuf::from("/second.txt"), second_path).unwrap();
        first.set_text("first edited".into());
        second.set_text("second edited".into());

        assert!(second.undo().is_some());
        assert_eq!(second.text, "second original");
        assert_eq!(first.text, "first edited");
        assert!(first.undo().is_some());
        assert_eq!(first.text, "first original");
        assert!(!first.dirty);
        assert!(!second.dirty);
        fs::remove_dir_all(&directory).unwrap();
    }
}
