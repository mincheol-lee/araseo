//! A terminal is one painted surface and one native IME client, independent of
//! the dock group containing it. Moving a tab never replaces its input handler.
use crate::{
    terminal::{TerminalFrame, TerminalSession, TerminalWheel},
    terminal_paint::{self, Composition},
};
use gpui_kit::{self as gpui, base::TestSupportExt, prelude::*, *};
use std::{ops::Range, sync::Arc, time::Duration};

pub fn init(cx: &mut App) {
    // Root's focus-navigation actions run before raw key listeners. Let the
    // terminal encode Tab/Backtab itself, while other controls keep navigation.
    cx.bind_keys([
        KeyBinding::new("tab", NoAction, Some("Terminal")),
        KeyBinding::new("shift-tab", NoAction, Some("Terminal")),
        // These are input-editing commands in agent TUIs (delete word / stash).
        KeyBinding::new("ctrl-w", NoAction, Some("Terminal")),
        KeyBinding::new("ctrl-s", NoAction, Some("Terminal")),
    ]);
}

#[derive(Clone)]
struct DrawRun {
    column: i32,
    columns: i32,
    line: ShapedLine,
    background: Hsla,
}
pub struct TerminalView {
    pub session: TerminalSession,
    focus: FocusHandle,
    pub frame: Arc<TerminalFrame>,
    composition: Composition,
    bounds: Bounds<Pixels>,
    pub font_size: f32,
    pub brightness: i32,
    cell_width: Pixels,
    line_height: Pixels,
    requested_size: (u16, u16),
    cache: Vec<(Option<Arc<crate::terminal::FrameRow>>, Vec<DrawRun>)>,
    selection: Option<((i32, i32), (i32, i32))>,
    selecting: bool,
    visible: bool,
    _poll: Task<()>,
    pub input_error: Option<String>,
}
impl TerminalView {
    pub fn new(
        session: TerminalSession,
        font_size: f32,
        brightness: i32,
        cx: &mut Context<Self>,
    ) -> Self {
        let frame = session.frame();
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        // Also drain resize completions when no new output arrives.
                        let (rows, columns) = this.requested_size;
                        this.session.resize(rows, columns);
                        if this.session.poll() {
                            this.frame = this.session.frame();
                            if this.visible {
                                cx.notify();
                            }
                        }
                        if let Some(error) = this.session.take_input_error() {
                            this.input_error = Some(error);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            session,
            focus: cx.focus_handle(),
            frame,
            composition: Composition::default(),
            bounds: Bounds::default(),
            font_size,
            brightness,
            cell_width: px(font_size * 0.6),
            line_height: px(font_size * 1.35),
            requested_size: (24, 80),
            cache: Vec::new(),
            selection: None,
            selecting: false,
            visible: true,
            _poll: task,
            input_error: None,
        }
    }
    pub fn set_visible(&mut self, active: bool, cx: &mut Context<Self>) {
        self.visible = active;
        self.session.set_visible(active);
        if active {
            self.frame = self.session.frame();
            cx.notify();
        }
    }
    pub fn set_font(&mut self, size: f32, brightness: i32, cx: &mut Context<Self>) {
        self.font_size = size;
        self.brightness = brightness;
        self.cache.clear();
        cx.notify();
    }
    pub fn send(&mut self, text: &str, cx: &mut Context<Self>) {
        self.selection = None;
        if !self.session.write(text.as_bytes()) {
            self.input_error = Some("Terminal input was not sent".into());
        }
        cx.notify();
    }
    fn cell_at(&self, position: Point<Pixels>) -> (i32, i32) {
        let row = ((position.y - self.bounds.top()) / self.line_height).floor() as i32;
        let column = ((position.x - self.bounds.left()) / self.cell_width).floor() as i32;
        (
            row.clamp(0, i32::from(self.frame.size.0) - 1),
            column.clamp(0, i32::from(self.frame.size.1) - 1),
        )
    }
    fn copy(&self, cx: &mut App) {
        if let Some((a, b)) = self.selection {
            let text = self.session.selection_text(a.0, a.1, b.0, b.1);
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke.key;
        let modifiers = event.keystroke.modifiers;
        if modifiers.control && modifiers.shift && key == "c" {
            self.copy(cx);
            cx.stop_propagation();
            return;
        }
        if (modifiers.control && !modifiers.alt && key == "v")
            || (modifiers.shift && key == "insert")
        {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.send(&text, cx);
            }
            cx.stop_propagation();
            window.prevent_default();
            return;
        }
        // The OS IME owns editing/navigation until composition has committed.
        if !self.composition.text.is_empty() {
            return;
        }
        // AltGr is represented as Ctrl+Alt on Windows. Let native input commit
        // its printable layout character instead of turning it into a shortcut.
        if modifiers.control
            && modifiers.alt
            && event.prefer_character_input
            && event
                .keystroke
                .key_char
                .as_ref()
                .is_some_and(|text| !text.is_empty() && text.chars().all(|ch| !ch.is_control()))
        {
            return;
        }
        let special = match key.as_str() {
            "space" if modifiers.control || modifiers.alt => Some(" "),
            "enter" => Some("<ENTER>"),
            "backspace" => Some("<BACKSPACE>"),
            "delete" => Some("<DELETE>"),
            "insert" => Some("<INSERT>"),
            "escape" => Some("<ESCAPE>"),
            "tab" if modifiers.shift => Some("<BACKTAB>"),
            "tab" => Some("<TAB>"),
            "up" => Some("<UP>"),
            "down" => Some("<DOWN>"),
            "left" => Some("<LEFT>"),
            "right" => Some("<RIGHT>"),
            "home" => Some("<HOME>"),
            "end" => Some("<END>"),
            "pageup" => Some("<PAGEUP>"),
            "pagedown" => Some("<PAGEDOWN>"),
            "f1" => Some("<F1>"),
            "f2" => Some("<F2>"),
            "f3" => Some("<F3>"),
            "f4" => Some("<F4>"),
            "f5" => Some("<F5>"),
            "f6" => Some("<F6>"),
            "f7" => Some("<F7>"),
            "f8" => Some("<F8>"),
            "f9" => Some("<F9>"),
            "f10" => Some("<F10>"),
            "f11" => Some("<F11>"),
            "f12" => Some("<F12>"),
            _ => None,
        };
        if modifiers.control && key == "c" && self.selection.is_some() {
            self.copy(cx);
            self.selection = None;
        } else if special.is_some() || modifiers.control || modifiers.alt {
            let text = special.unwrap_or(key);
            self.session.write(&crate::terminal::encode_key(
                text,
                modifiers.control,
                modifiers.alt,
                modifiers.shift,
            ));
        } else {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
    }
    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Vec<(Point<Pixels>, Vec<DrawRun>)> {
        let mut font = font("Cascadia Mono");
        font.features = FontFeatures::disable_ligatures();
        let width = window
            .text_system()
            .shape_line(
                "M".into(),
                px(self.font_size),
                &[TextRun {
                    len: 1,
                    font: font.clone(),
                    color: rgb(0xffffff).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width();
        if self.cell_width != width {
            self.cache.clear();
        }
        self.cell_width = width.max(px(1.));
        self.line_height = px(self.font_size * 1.35);
        self.bounds = bounds;
        let rows = (bounds.size.height / self.line_height)
            .floor()
            .max(1.)
            .min(500.) as u16;
        let columns = (bounds.size.width / self.cell_width)
            .floor()
            .max(1.)
            .min(1000.) as u16;
        self.requested_size = (rows, columns);
        self.session.resize(rows, columns);
        let mut painted = Vec::new();
        for (row, frame_row) in self.frame.rows.iter().enumerate().take(usize::from(rows)) {
            if self.cache.len() <= row {
                self.cache.push((None, Vec::new()));
            }
            if self.cache[row]
                .0
                .as_ref()
                .is_none_or(|cached| !Arc::ptr_eq(cached, frame_row))
            {
                let runs = terminal_paint::runs(&frame_row.cells)
                    .into_iter()
                    .map(|run| {
                        let color = |rgb: [u8; 3]| -> Hsla {
                            let [r, g, b] = rgb.map(|v| {
                                (i32::from(v) * self.brightness / 120).clamp(0, 255) as u32
                            });
                            gpui::rgb(r << 16 | g << 8 | b).into()
                        };
                        let (foreground, background) =
                            if run.cursor && self.composition.text.is_empty() {
                                (rgb(0x101820).into(), rgb(0xeeeeee).into())
                            } else {
                                (color(run.foreground), color(run.background))
                            };
                        let mut font = font.clone();
                        if run.bold {
                            font.weight = FontWeight::BOLD;
                        }
                        let line = window.text_system().shape_line(
                            run.text.clone().into(),
                            px(self.font_size),
                            &[TextRun {
                                len: run.text.len(),
                                font,
                                color: foreground,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            // GPUI's forced-width shaper advances per base glyph,
                            // so two-column graphemes must be positioned separately.
                            if run.single_width {
                                Some(self.cell_width)
                            } else {
                                None
                            },
                        );
                        DrawRun {
                            column: run.column,
                            columns: run.columns,
                            line,
                            background,
                        }
                    })
                    .collect();
                self.cache[row] = (Some(frame_row.clone()), runs);
            }
            painted.push((
                point(bounds.left(), bounds.top() + self.line_height * row as f32),
                self.cache[row].1.clone(),
            ));
        }
        self.cache.truncate(self.frame.rows.len());
        painted
    }
    fn caret_bounds(&self) -> Bounds<Pixels> {
        let (row, col) = self.frame.cursor.unwrap_or((0, 0));
        Bounds::new(
            point(
                self.bounds.left() + self.cell_width * f32::from(col),
                self.bounds.top() + self.line_height * f32::from(row),
            ),
            size(self.cell_width, self.line_height),
        )
    }
}
impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prepare = cx.entity();
        let paint = cx.entity();
        div()
            .id("terminal-surface")
            .test_support()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x282c34))
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_drop(
                cx.listener(|this, path: &crate::gpui_app::DragPath, window, cx| {
                    this.focus.focus(window, cx);
                    let path = path.0.to_string_lossy();
                    this.send(&format!(" '{}' ", path.replace('\'', "'\\''")), cx);
                }),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let _distro = this.session.distribution();
                let text = paths
                    .paths()
                    .iter()
                    .map(|path| {
                        #[cfg(windows)]
                        let path = crate::workspace::dropped_file_linux_path(_distro, path)
                            .map_err(|e| e.to_string())?;
                        #[cfg(not(windows))]
                        let path = path.clone();
                        Ok(format!(
                            "'{}'",
                            path.to_string_lossy().replace('\'', "'\\''")
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>();
                match text {
                    Ok(paths) => {
                        this.focus.focus(window, cx);
                        this.send(&format!(" {} ", paths.join(" ")), cx);
                    }
                    Err(error) => {
                        this.input_error = Some(error);
                        cx.notify();
                    }
                }
            }))
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus.focus(window, cx);
                    let cell = this.cell_at(event.position);
                    this.selection = Some((cell, cell));
                    this.selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.selecting {
                    let cell = this.cell_at(event.position);
                    if let Some((_, end)) = &mut this.selection {
                        *end = cell;
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.selecting = false;
                    if this.selection.is_some_and(|(a, b)| a == b) {
                        this.selection = None;
                    }
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                    if this.selection.is_some() {
                        this.copy(cx);
                    } else if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                    {
                        this.send(&text, cx);
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(this.line_height);
                let (row, column) = this.cell_at(event.position);
                // GPUI reports upward wheel motion as positive, matching both
                // local history offsets and the terminal's wheel-up report.
                let rows = (delta.y / this.line_height).round() as i32;
                this.session.scroll_scrollback(TerminalWheel {
                    rows,
                    row,
                    column,
                    control: event.modifiers.control,
                    alt: event.modifiers.alt,
                    shift: event.modifiers.shift,
                });
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        prepare.update(cx, |this, cx| this.prepare(bounds, window, cx))
                    },
                    move |bounds, rows, window, cx| {
                        let view = paint.read(cx);
                        let focus = view.focus.clone();
                        let height = view.line_height;
                        let width = view.cell_width;
                        let composition = view.composition.text.clone();
                        let caret = view.caret_bounds();
                        let selection = view.selection;
                        let font_size = view.font_size;
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            for (origin, runs) in rows {
                                for run in runs {
                                    let origin =
                                        point(origin.x + width * run.column as f32, origin.y);
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            origin,
                                            size(width * run.columns as f32, height),
                                        ),
                                        run.background,
                                    ));
                                    let _ = run.line.paint(
                                        origin,
                                        height,
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                }
                            }
                            if let Some((a, b)) = selection {
                                let (a, b) = if a <= b { (a, b) } else { (b, a) };
                                for row in a.0..=b.0 {
                                    let start = if row == a.0 { a.1 } else { 0 };
                                    let end = if row == b.0 {
                                        b.1 + 1
                                    } else {
                                        (bounds.size.width / width) as i32
                                    };
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(
                                                bounds.left() + width * start as f32,
                                                bounds.top() + height * row as f32,
                                            ),
                                            size(width * (end - start) as f32, height),
                                        ),
                                        rgba(0x6090c050),
                                    ));
                                }
                            }
                            if !composition.is_empty() {
                                let line = window.text_system().shape_line(
                                    composition.clone().into(),
                                    px(font_size),
                                    &[TextRun {
                                        len: composition.len(),
                                        font: font("Cascadia Mono"),
                                        color: rgb(0xffffff).into(),
                                        background_color: Some(rgb(0x303844).into()),
                                        underline: Some(UnderlineStyle {
                                            thickness: px(1.),
                                            color: None,
                                            wavy: false,
                                        }),
                                        strikethrough: None,
                                    }],
                                    None,
                                );
                                let _ = line.paint(
                                    caret.origin,
                                    height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        });
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint.clone()),
                            cx,
                        );
                    },
                )
                .size_full(),
            )
    }
}
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (text, range) = self.composition.slice(range);
        *actual = Some(range);
        Some(text)
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.composition.selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composition.text.is_empty()).then(|| 0..self.composition.text.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.composition.clear();
        self.cache.clear();
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition.clear();
        self.cache.clear();
        self.send(text, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition.mark(text, selection);
        self.cache.clear();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.caret_bounds())
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}
