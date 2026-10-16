//! Framework independent terminal paint runs and IME composition state.
//! Adjacent cells share shaping work, while wide glyphs retain their grid span.
use crate::terminal::DisplayCell;
use std::ops::Range;

#[derive(Debug, PartialEq, Eq)]
pub struct PaintRun {
    pub column: i32,
    pub columns: i32,
    pub single_width: bool,
    pub text: String,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub bold: bool,
    pub cursor: bool,
}

pub fn runs(cells: &[DisplayCell]) -> Vec<PaintRun> {
    let mut runs: Vec<PaintRun> = Vec::new();
    for cell in cells {
        if let Some(last) = runs.last_mut()
            && last.single_width
            && cell.column_span == 1
            && last.column + last.columns == cell.column
            && last.foreground == cell.foreground
            && last.background == cell.background
            && last.bold == cell.bold
            && last.cursor == cell.cursor
        {
            last.text.push_str(&cell.glyph);
            last.columns += cell.column_span;
        } else {
            runs.push(PaintRun {
                column: cell.column,
                columns: cell.column_span,
                single_width: cell.column_span == 1,
                text: cell.glyph.clone(),
                foreground: cell.foreground,
                background: cell.background,
                bold: cell.bold,
                cursor: cell.cursor,
            });
        }
    }
    runs
}

/// Only uncommitted preedit belongs to the GUI. Committed text is sent once to
/// the PTY and is subsequently displayed from the terminal's own output.
#[derive(Default, Debug)]
pub struct Composition {
    pub text: String,
    pub selection: Range<usize>,
}
impl Composition {
    pub fn mark(&mut self, text: &str, selection: Option<Range<usize>>) {
        self.text = text.into();
        let len = text.encode_utf16().count();
        let selection = selection.unwrap_or(len..len);
        self.selection = selection.start.min(len)..selection.end.min(len);
    }
    pub fn clear(&mut self) {
        self.text.clear();
        self.selection = 0..0;
    }
    pub fn slice(&self, range: Range<usize>) -> (String, Range<usize>) {
        fn boundary(text: &str, target: usize) -> (usize, usize) {
            let mut utf16 = 0;
            for (byte, ch) in text.char_indices() {
                if utf16 + ch.len_utf16() > target {
                    return (byte, utf16);
                }
                utf16 += ch.len_utf16();
            }
            (text.len(), utf16)
        }
        let (start, actual_start) = boundary(&self.text, range.start);
        let (end, actual_end) = boundary(&self.text, range.end.max(range.start));
        (self.text[start..end].into(), actual_start..actual_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batches_only_adjacent_compatible_cells_and_preserves_wide_spans() {
        let mut cells: Vec<_> = ["a", "b", "한", "c", "d"]
            .into_iter()
            .zip([0, 1, 2, 4, 5])
            .map(|(glyph, column)| DisplayCell {
                row: 0,
                column,
                glyph: glyph.into(),
                foreground: [255; 3],
                background: [0; 3],
                bold: false,
                cursor: false,
                column_span: if glyph == "한" { 2 } else { 1 },
            })
            .collect();
        let result = runs(&cells);
        assert_eq!(
            result
                .iter()
                .map(|run| run.text.as_str())
                .collect::<Vec<_>>(),
            vec!["ab", "한", "cd"]
        );
        assert_eq!(result[1].column, 2);
        assert_eq!(result[1].columns, 2);
        assert!(!result[1].single_width);
        assert_eq!(result[2].column, 4);
        cells[1].cursor = true;
        assert_eq!(runs(&cells).len(), 4);
        cells[1].cursor = false;
        cells[4].column = 8;
        assert_eq!(runs(&cells).len(), 4);
    }
    #[test]
    fn preedit_is_replaced_and_utf16_surrogate_ranges_never_panic() {
        let mut composition = Composition::default();
        composition.mark("ㅎ", None);
        composition.mark("한😀글", Some(99..100));
        assert_eq!(composition.selection, 4..4);
        assert_eq!(composition.slice(1..2), (String::new(), 1..1));
        assert_eq!(composition.slice(1..3), ("😀".into(), 1..3));
        composition.clear();
        assert_eq!(composition.slice(0..99), (String::new(), 0..0));
    }
}
