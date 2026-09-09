use crate::{background::Background, highlight};
use slint::{SharedString, StyledText};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Presentation work is delayed until typing pauses, and unchanged panes keep
/// their existing layout. A changed buffer immediately invalidates old colors.
#[derive(Default)]
pub struct EditorView {
    path: PathBuf,
    text: SharedString,
    line_count: usize,
    brightness: i32,
    deadline: Option<Instant>,
    worker: Background<Option<String>>,
}

impl EditorView {
    pub fn update(
        &mut self,
        path: &Path,
        text: SharedString,
        brightness: i32,
        now: Instant,
    ) -> (bool, Option<SharedString>) {
        let content_changed = self.path != path || self.text != text;
        let brightness = brightness.clamp(50, 150);
        if !content_changed && self.brightness == brightness {
            return (false, None);
        }
        self.path = path.to_path_buf();
        self.text = text;
        self.brightness = brightness;
        self.worker.cancel();
        self.deadline = Some(now + Duration::from_millis(120));
        let lines = self.text.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let numbers = if content_changed && self.line_count != lines {
            self.line_count = lines;
            Some(
                (1..=lines)
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into(),
            )
        } else {
            None
        };
        (true, numbers)
    }

    pub fn poll(&mut self, now: Instant) -> Option<Option<StyledText>> {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
            let path = self.path.clone();
            let text = self.text.to_string();
            let brightness = self.brightness;
            self.worker
                .request(move || highlight::markup_with_brightness(&path, &text, brightness));
        }
        self.worker
            .poll()
            .map(|markup| markup.and_then(|markup| StyledText::from_markdown(&markup).ok()))
    }

    pub fn clear(&mut self) {
        self.path.clear();
        self.text = SharedString::default();
        self.line_count = 0;
        self.brightness = 0;
        self.deadline = None;
        self.worker.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual before/after presentation microbenchmark; not an end-to-end latency test"]
    fn benchmark_edit_presentation() {
        let path = Path::new("sample.rs");
        let source =
            "fn example() { let message = \"한글\"; println!(\"{}\", message); }\n".repeat(200);
        let edits: Vec<SharedString> = (0..100)
            .map(|index| format!("{source}// edit {index}").into())
            .collect();
        let before = Instant::now();
        for text in &edits {
            std::hint::black_box(highlight::highlighted(path, text));
            let count = text.bytes().filter(|byte| *byte == b'\n').count() + 1;
            std::hint::black_box(
                (1..=count)
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        let previous = before.elapsed();
        let mut view = EditorView::default();
        let now = Instant::now();
        let before = Instant::now();
        for text in edits {
            std::hint::black_box(view.update(
                path,
                text,
                crate::appearance::DEFAULT_FONT_BRIGHTNESS,
                now,
            ));
        }
        eprintln!(
            "100 edits, {} bytes: previous presentation {:?}, deferred input path {:?} (excludes document edits, rendering and eventual highlighting)",
            source.len(),
            previous,
            before.elapsed()
        );
    }

    #[test]
    fn typing_defers_colors_and_only_rebuilds_changed_line_numbers() {
        let mut view = EditorView::default();
        let now = Instant::now();
        let (changed, numbers) = view.update(
            Path::new("a.rs"),
            "let a = 1;".into(),
            crate::appearance::DEFAULT_FONT_BRIGHTNESS,
            now,
        );
        assert!(changed);
        assert_eq!(numbers.unwrap(), "1");
        assert!(view.poll(now + Duration::from_millis(119)).is_none());
        let (changed, numbers) = view.update(
            Path::new("a.rs"),
            "let a = 12;".into(),
            crate::appearance::DEFAULT_FONT_BRIGHTNESS,
            now + Duration::from_millis(100),
        );
        assert!(changed);
        assert!(numbers.is_none());
        assert!(view.poll(now + Duration::from_millis(200)).is_none());
        assert_eq!(
            view.update(
                Path::new("a.rs"),
                "let a = 12;".into(),
                crate::appearance::DEFAULT_FONT_BRIGHTNESS,
                now,
            )
            .0,
            false
        );
        let (_, numbers) = view.update(
            Path::new("a.rs"),
            "let a = 12;\n".into(),
            crate::appearance::DEFAULT_FONT_BRIGHTNESS,
            now,
        );
        assert_eq!(numbers.unwrap(), "1\n2");
    }

    #[test]
    fn switching_files_cancels_old_colors_and_identical_text_is_reclassified() {
        let mut view = EditorView::default();
        let now = Instant::now();
        view.update(
            Path::new("a.rs"),
            "let a = 1;".into(),
            crate::appearance::DEFAULT_FONT_BRIGHTNESS,
            now,
        );
        view.poll(now + Duration::from_secs(1));
        assert!(
            view.update(
                Path::new("a.txt"),
                "let a = 1;".into(),
                crate::appearance::DEFAULT_FONT_BRIGHTNESS,
                now,
            )
            .0
        );
        assert!(view.poll(now).is_none());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(colors) = view.poll(now + Duration::from_secs(1)) {
                assert!(colors.is_none());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        view.clear();
        assert!(view.poll(now + Duration::from_secs(2)).is_none());
    }

    #[test]
    fn brightness_change_rebuilds_colors_without_rebuilding_line_numbers() {
        let mut view = EditorView::default();
        let now = Instant::now();
        view.update(Path::new("a.rs"), "let a = 1;\n".into(), 120, now);
        let (changed, numbers) = view.update(Path::new("a.rs"), "let a = 1;\n".into(), 135, now);
        assert!(changed);
        assert!(numbers.is_none());
    }
}
