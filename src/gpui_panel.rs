//! Dockable application content. Every tab owns its editor or terminal entity;
//! docking changes its container, never its running session or edit buffer.
use crate::{
    background::Background,
    document::Document,
    document_io,
    gpui_agent_icon::AgentTabIcon,
    gpui_terminal::TerminalView,
    preview,
    session::{SavedKind, SavedTab},
    tabs::TabId,
    terminal::{AgentKind, TerminalSession},
};
use gpui_kit::component::{
    ElementExt, Sizable,
    button::{Button, ButtonVariants},
    diff::{Diff, DiffFile, DiffMode, DiffState},
    dock::{BasePanel, Panel, PanelEvent, PanelInfo, PanelState},
    input::{EditorState, InputEvent},
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    text::{TextView, TextViewState},
};
use gpui_kit::{base::TestSupportExt, prelude::*, *};
use std::{path::PathBuf, sync::Arc, time::Duration};

pub enum Opened {
    File(Document),
    Preview(PreparedPreview),
    Diff(crate::git::FileDiff),
    Terminal(TerminalSession),
}
/// The expensive pixel conversion runs alongside decoding on the worker.
pub struct PreparedPreview {
    loaded: preview::LoadedPreview,
    image: Arc<RenderImage>,
    width: f32,
    height: f32,
    text: preview::PdfTextPage,
}
impl PreparedPreview {
    pub fn new(mut loaded: preview::LoadedPreview) -> Self {
        let natural = loaded
            .first_page
            .as_ref()
            .map(|page| (page.page_width, page.page_height));
        let (pixels, text) = if let Some(page) = loaded.first_page.take() {
            (
                preview::ImagePixels {
                    width: page.width,
                    height: page.height,
                    rgba: page.rgba,
                    premultiplied: false,
                },
                page.text,
            )
        } else {
            (
                loaded.pixels.take().expect("decoded preview pixels"),
                preview::PdfTextPage::default(),
            )
        };
        let (width, height) = natural.unwrap_or((pixels.width as f32, pixels.height as f32));
        Self {
            loaded,
            image: render_image(pixels),
            width,
            height,
            text,
        }
    }
}
struct PreparedPage {
    image: Arc<RenderImage>,
    width: f32,
    height: f32,
    text: preview::PdfTextPage,
}
impl From<preview::RenderedPage> for PreparedPage {
    fn from(page: preview::RenderedPage) -> Self {
        let width = page.page_width;
        let height = page.page_height;
        Self {
            image: render_image(preview::ImagePixels {
                width: page.width,
                height: page.height,
                rgba: page.rgba,
                premultiplied: false,
            }),
            width,
            height,
            text: page.text,
        }
    }
}
pub enum Content {
    Loading,
    File {
        document: Document,
        editor: Entity<EditorState>,
        markdown: Option<Entity<TextViewState>>,
        source: bool,
    },
    Preview {
        image: Arc<RenderImage>,
        bytes: Option<Arc<Vec<u8>>>,
        page: usize,
        count: usize,
        zoom: f32,
        width: f32,
        height: f32,
        text: preview::PdfTextPage,
    },
    Diff(Entity<DiffState>),
    Terminal(Entity<TerminalView>),
    Error(String),
}
#[derive(Clone)]
pub enum ContentEvent {
    Navigate(
        crate::code_navigation::Location,
        crate::code_navigation::Position,
    ),
    Closed(TabId),
    RequestClose(TabId),
    RequestCloseFiles(Option<TabId>),
    Changed,
    Focused(TabId),
    Message(String),
    RefreshTree,
}
pub struct WorkspacePanel {
    pending_visit: Option<crate::code_navigation::Visit>,
    pub(crate) navigation_path: Option<PathBuf>,
    pub id: TabId,
    pub path: PathBuf,
    pub kind: SavedKind,
    pub content: Content,
    pub title: String,
    pub dirty: bool,
    pub active: bool,
    agent: Option<AgentKind>,
    agent_icon: Entity<AgentTabIcon>,
    focus: FocusHandle,
    pub font_size: f32,
    loader: Background<Result<Opened, String>>,
    io: Background<document_io::Outcome>,
    refresh: Background<(document_io::Probe, Result<Option<Document>, String>)>,
    page_loader: Background<Result<(usize, f32, PreparedPage), String>>,
    conflict: bool,
    subscriptions: Vec<Subscription>,
    _poll: Task<()>,
    refresh_tick: u16,
    brightness: i32,
    preview_bounds: Bounds<Pixels>,
    preview_viewport: Size<Pixels>,
    diff_viewport: Size<Pixels>,
    preview_fit: bool,
    preview_scale: f32,
    pdf_selection: Option<(usize, usize)>,
    selecting_pdf: bool,
}
impl WorkspacePanel {
    pub fn has_unsaved_changes(&self) -> bool {
        self.dirty
            || (matches!(self.content, Content::Loading | Content::Error(_))
                && matches!(
                    self.kind,
                    SavedKind::File {
                        unsaved_text: Some(_),
                        ..
                    }
                ))
    }

    pub fn discard_unsaved_changes(&mut self) {
        self.dirty = false;
        if let SavedKind::File {
            unsaved_text,
            revision,
        } = &mut self.kind
        {
            *unsaved_text = None;
            *revision = None;
        }
    }

    fn file_menu(menu: PopupMenu, panel: WeakEntity<Self>, id: TabId) -> PopupMenu {
        let close = panel.clone();
        let others = panel.clone();
        menu.item(PopupMenuItem::new("Close File").on_click(move |_, _, cx| {
            let _ = close.update(cx, |_, cx| cx.emit(ContentEvent::RequestClose(id)));
        }))
        .separator()
        .item(
            PopupMenuItem::new("Close Other Files").on_click(move |_, _, cx| {
                let _ = others.update(cx, |_, cx| {
                    cx.emit(ContentEvent::RequestCloseFiles(Some(id)))
                });
            }),
        )
        .item(
            PopupMenuItem::new("Close All Files").on_click(move |_, _, cx| {
                let _ = panel.update(cx, |_, cx| cx.emit(ContentEvent::RequestCloseFiles(None)));
            }),
        )
    }

    pub fn new(tab: SavedTab, font_size: f32, cx: &mut Context<Self>) -> Self {
        let title = match tab.kind {
            SavedKind::Terminal { number } => format!("Terminal {number}"),
            SavedKind::Diff => format!(
                "Diff · {}",
                tab.path.file_name().unwrap_or_default().to_string_lossy()
            ),
            _ => tab
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        };
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            pending_visit: None,
            navigation_path: None,
            id: tab.id,
            path: tab.path,
            kind: tab.kind,
            content: Content::Loading,
            title,
            dirty: false,
            active: false,
            agent: None,
            agent_icon: cx.new(AgentTabIcon::new),
            focus: cx.focus_handle(),
            font_size,
            loader: Background::default(),
            io: Background::default(),
            refresh: Background::default(),
            page_loader: Background::default(),
            conflict: false,
            subscriptions: vec![],
            _poll: task,
            refresh_tick: 0,
            brightness: 120,
            preview_bounds: Bounds::default(),
            preview_viewport: Size::default(),
            diff_viewport: Size::default(),
            preview_fit: true,
            preview_scale: 1.,
            pdf_selection: None,
            selecting_pdf: false,
        }
    }
    #[cfg(feature = "ui-tests")]
    pub fn measured_preview_bounds(&self) -> Bounds<Pixels> {
        self.preview_bounds
    }
    pub fn load(&mut self, job: impl FnOnce() -> Result<Opened, String> + Send + 'static) {
        self.loader.request(job);
    }
    pub fn finish(&mut self, opened: Opened, window: &mut Window, cx: &mut Context<Self>) {
        self.content = match opened {
            Opened::Terminal(session) => Content::Terminal(
                cx.new(|cx| TerminalView::new(session, self.font_size, self.brightness, cx)),
            ),
            Opened::File(mut document) => {
                self.kind.restore_file(&mut document);
                self.dirty = document.dirty;
                let language = match self.path.extension().and_then(|s| s.to_str()).unwrap_or("") {
                    "rs" => "rust",
                    "py" => "python",
                    "js" => "javascript",
                    "ts" => "typescript",
                    "md" => "markdown",
                    "toml" => "toml",
                    "json" => "json",
                    "sh" => "bash",
                    _ => "plain",
                };
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language(language)
                        .line_number(true)
                        .soft_wrap(false)
                        .default_value(document.text.clone())
                });
                let markdown = (language == "markdown")
                    .then(|| cx.new(|cx| TextViewState::markdown(&document.text, cx)));
                self.subscriptions.push(cx.subscribe(
                    &editor,
                    |this, editor, event: &InputEvent, cx| match event {
                        InputEvent::Change => {
                            if let Content::File {
                                document, markdown, ..
                            } = &mut this.content
                            {
                                document.set_text(editor.read(cx).value().to_string());
                                this.dirty = document.dirty;
                                if let Some(markdown) = markdown {
                                    markdown
                                        .update(cx, |state, cx| state.set_text(&document.text, cx));
                                }
                                cx.emit(ContentEvent::Changed);
                                cx.emit(PanelEvent::LayoutChanged);
                                cx.notify();
                            }
                        }
                        InputEvent::Focus => cx.emit(ContentEvent::Focused(this.id)),
                        _ => {}
                    },
                ));
                Content::File {
                    document,
                    editor,
                    markdown,
                    source: language != "markdown",
                }
            }
            Opened::Diff(diff) => match DiffFile::parse(&diff.patch) {
                Ok(files) if !files.is_empty() => {
                    let state = cx.new(|cx| {
                        let mut state = DiffState::new(files, cx);
                        state.set_mode(DiffMode::Split, cx);
                        state.set_context_lines(None, cx);
                        state
                    });
                    // Diff is a RenderOnce component backed by an entity. Its
                    // viewport, selection and background syntax updates must
                    // invalidate this owning panel, including the first paint.
                    self.subscriptions
                        .push(cx.observe(&state, |_, _, cx| cx.notify()));
                    Content::Diff(state)
                }
                _ => Content::Error("Binary file or empty change".into()),
            },
            Opened::Preview(prepared) => Content::Preview {
                image: prepared.image,
                bytes: prepared.loaded.pdf_bytes.map(Arc::new),
                page: 0,
                count: prepared.loaded.page_count,
                zoom: 1.,
                width: prepared.width,
                height: prepared.height,
                text: prepared.text,
            },
        };
        let focus = self.focus_handle(cx);
        self.apply_visit(window, cx);
        self.subscriptions
            .push(cx.on_focus(&focus, window, |this, _, cx| {
                cx.emit(ContentEvent::Focused(this.id))
            }));
        if self.active {
            focus.focus(window, cx);
        }
        cx.emit(ContentEvent::Changed);
        cx.notify();
    }
    pub fn visit(&self, cx: &App) -> Option<crate::code_navigation::Visit> {
        let Content::File {
            editor, document, ..
        } = &self.content
        else {
            return None;
        };
        let editor = editor.read(cx);
        let range = editor.selected_range();
        let scroll = editor.scroll_offset();
        Some(crate::code_navigation::Visit {
            tab: Some(self.id),
            location: crate::code_navigation::Location {
                path: self.path.clone(),
                range: crate::code_navigation::Range {
                    start: crate::code_navigation::position(&document.text, range.start),
                    end: crate::code_navigation::position(&document.text, range.end),
                },
            },
            scroll: Some((f32::from(scroll.x), f32::from(scroll.y))),
        })
    }
    pub fn navigate(
        &mut self,
        visit: crate::code_navigation::Visit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_visit = Some(visit);
        self.apply_visit(window, cx);
    }
    fn apply_visit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Content::File {
            document,
            editor,
            source,
            ..
        } = &mut self.content
        else {
            return;
        };
        let Some(visit) = self.pending_visit.take() else {
            return;
        };
        *source = true;
        let start = crate::code_navigation::offset(&document.text, visit.location.range.start);
        let end = crate::code_navigation::offset(&document.text, visit.location.range.end);
        let active = self.active;
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(start..end, cx);
            if let Some((x, y)) = visit.scroll {
                editor.set_scroll_offset(point(px(x), px(y)), cx);
            }
            if active {
                editor.focus(window, cx);
            }
        });
        cx.notify();
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        if let Some(result) = self.loader.poll() {
            // GPUI's async Context carries no Window. Finish at the next window
            // update so editor and native input state initialize on the UI thread.
            let entity = cx.entity();
            cx.defer(move |cx| {
                if let Some(window) = cx.windows().first().copied() {
                    let _ = window.update(cx, |_, window, cx| {
                        entity.update(cx, |this, cx| match result {
                            Ok(opened) => this.finish(opened, window, cx),
                            Err(error) => {
                                this.content = Content::Error(error);
                                cx.notify();
                            }
                        })
                    });
                }
            });
        }
        if let Some(outcome) = self.io.poll() {
            match outcome.result {
                Ok(saved) => {
                    if let Content::File {
                        document, editor, ..
                    } = &mut self.content
                    {
                        if document.host_path != outcome.host
                            || (matches!(outcome.operation, document_io::Operation::Reload)
                                && document.edit_version() != outcome.version)
                        {
                            cx.emit(ContentEvent::Message("File operation completed after newer edits; the current buffer was kept".into()));
                            return;
                        }
                        match outcome.operation {
                            document_io::Operation::Save(_) => document.accept_saved(&saved),
                            document_io::Operation::Reload => {
                                document.accept_reloaded(saved);
                                let text = document.text.clone();
                                let editor = editor.clone();
                                cx.defer(move |cx| {
                                    if let Some(window) = cx.windows().first().copied() {
                                        let _ = window.update(cx, |_, window, cx| {
                                            editor.update(cx, |state, cx| {
                                                state.set_value(text, window, cx)
                                            })
                                        });
                                    }
                                });
                            }
                        }
                        self.dirty = document.dirty;
                        self.conflict = false;
                        cx.emit(ContentEvent::Message("File saved/refreshed".into()));
                    }
                }
                Err(error) => {
                    self.conflict = outcome.conflict;
                    cx.emit(ContentEvent::Message(error));
                }
            }
            cx.emit(ContentEvent::Changed);
            cx.notify();
        }
        if let Some((probe, result)) = self.refresh.poll() {
            if let Content::File {
                document, editor, ..
            } = &mut self.content
            {
                if probe.is_current(document) {
                    match result {
                        Ok(Some(loaded)) => {
                            document.accept_reloaded(loaded);
                            let text = document.text.clone();
                            let editor = editor.clone();
                            cx.defer(move |cx| {
                                if let Some(window) = cx.windows().first().copied() {
                                    let _ = window.update(cx, |_, window, cx| {
                                        editor.update(cx, |state, cx| {
                                            state.set_value(text, window, cx)
                                        })
                                    });
                                }
                            });
                        }
                        Err(error) => {
                            self.conflict = true;
                            cx.emit(ContentEvent::Message(error));
                        }
                        _ => {}
                    }
                }
            }
            cx.notify();
        }
        if let Some(result) = self.page_loader.poll() {
            match result {
                Ok((index, zoom, page)) => {
                    if let Content::Preview {
                        image,
                        page: current,
                        zoom: current_zoom,
                        width,
                        height,
                        text,
                        ..
                    } = &mut self.content
                    {
                        *width = page.width * zoom;
                        *height = page.height * zoom;
                        *text = page.text;
                        *image = page.image;
                        *current = index;
                        *current_zoom = zoom;
                    }
                }
                Err(error) => cx.emit(ContentEvent::Message(error)),
            }
            self.pdf_selection = None;
            cx.notify();
        }
        self.refresh_tick = self.refresh_tick.wrapping_add(1);
        if self.refresh_tick % 60 == 0 && self.active && !self.dirty {
            if let Content::File { document, .. } = &self.content {
                let probe = document_io::Probe::new(self.id, document);
                self.refresh.request(move || probe.check());
            }
        }
        for error in [
            self.loader.take_error(),
            self.io.take_error(),
            self.refresh.take_error(),
            self.page_loader.take_error(),
        ]
        .into_iter()
        .flatten()
        {
            cx.emit(ContentEvent::Message(error));
        }
    }
    pub fn save(&mut self, overwrite: bool, cx: &mut Context<Self>) {
        if let Content::File {
            document, editor, ..
        } = &mut self.content
        {
            document.set_text(editor.read(cx).value().to_string());
            self.dirty = document.dirty;
            let document = document.save_snapshot();
            let id = self.id;
            self.io.request(move || {
                document_io::run(id, document, document_io::Operation::Save(overwrite))
            });
        }
    }
    fn reload(&mut self) {
        if let Content::File { document, .. } = &self.content {
            let document = document.save_snapshot();
            let id = self.id;
            self.io
                .request(move || document_io::run(id, document, document_io::Operation::Reload));
        }
    }
    pub fn saved(&self) -> SavedTab {
        SavedTab {
            id: self.id,
            path: self.path.clone(),
            kind: match &self.content {
                Content::File { document, .. } => SavedKind::file(document),
                _ => self.kind.clone(),
            },
        }
    }
    pub fn set_font(&mut self, size: f32, brightness: i32, cx: &mut Context<Self>) {
        self.font_size = size;
        self.brightness = brightness;
        if let Content::Terminal(terminal) = &self.content {
            terminal.update(cx, |this, cx| this.set_font(size, brightness, cx));
        }
        cx.notify();
    }
    pub fn set_agent(&mut self, status: Option<AgentKind>, cx: &mut Context<Self>) {
        if self.agent == status {
            return;
        }
        self.agent = status;
        self.agent_icon
            .update(cx, |icon, cx| icon.set_kind(status, cx));
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }
    fn select_pdf(&mut self, position: Point<Pixels>, start: bool, cx: &mut Context<Self>) {
        if let Content::Preview { text, .. } = &self.content {
            let x =
                f32::from(position.x - self.preview_bounds.left()) / self.preview_scale.max(0.001);
            let y =
                f32::from(position.y - self.preview_bounds.top()) / self.preview_scale.max(0.001);
            if let Some(glyph) = text.nearest_glyph(x, y) {
                if start || self.pdf_selection.is_none() {
                    self.pdf_selection = Some((glyph, glyph));
                } else if let Some((_, focus)) = &mut self.pdf_selection {
                    *focus = glyph;
                }
                cx.notify();
            }
        }
    }
    fn copy_pdf(&self, cx: &mut App) {
        if let (Content::Preview { text, .. }, Some((anchor, focus))) =
            (&self.content, self.pdf_selection)
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.selected_text(anchor, focus)));
        }
    }
    fn page(&mut self, delta: i32, zoom: f32) {
        self.pdf_selection = None;
        if let Content::Preview { zoom: current, .. } = &self.content {
            if *current != zoom {
                self.preview_fit = false;
            }
        }
        if let Content::Preview {
            bytes: Some(bytes),
            page,
            count,
            ..
        } = &self.content
        {
            let index = (*page as i32 + delta).clamp(0, count.saturating_sub(1) as i32) as usize;
            let bytes = bytes.clone();
            self.page_loader.request(move || {
                preview::render_page(&bytes, index, zoom)
                    .map(|page| (index, zoom, PreparedPage::from(page)))
                    .map_err(|e| format!("{e:#}"))
            });
        }
    }
}
fn render_image(mut pixels: preview::ImagePixels) -> Arc<RenderImage> {
    for pixel in pixels.rgba.chunks_exact_mut(4) {
        if pixels.premultiplied && pixel[3] > 0 {
            for channel in 0..3 {
                pixel[channel] =
                    (u32::from(pixel[channel]) * 255 / u32::from(pixel[3])).min(255) as u8;
            }
        }
        pixel.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba)
        .expect("validated preview dimensions");
    Arc::new(RenderImage::new(vec![image::Frame::new(image)]))
}
impl EventEmitter<PanelEvent> for WorkspacePanel {}
impl EventEmitter<ContentEvent> for WorkspacePanel {}
impl Focusable for WorkspacePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.content {
            Content::File { editor, .. } => editor.focus_handle(cx),
            Content::Terminal(terminal) => terminal.focus_handle(cx),
            Content::Diff(state) => state.focus_handle(cx),
            _ => self.focus.clone(),
        }
    }
}
impl BasePanel for WorkspacePanel {
    fn panel_name(&self) -> &'static str {
        "AraseoTab"
    }
    fn closable(&self, _: &App) -> bool {
        !self.has_unsaved_changes()
    }
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        if let Content::Terminal(terminal) = &self.content {
            terminal.update(cx, |this, cx| this.set_visible(active, cx));
        }
        if active {
            self.focus_handle(cx).focus(window, cx);
            cx.emit(ContentEvent::Focused(self.id));
        }
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ContentEvent::Closed(self.id));
    }
    fn dump(&self, _: &App) -> PanelState {
        PanelState {
            panel_name: "AraseoTab".into(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({"id":self.id})),
        }
    }
}
impl Panel for WorkspacePanel {
    // Use the custom title in multi-tab strips too. A plain tab_name bypasses
    // its context menu, dirty-close control, and agent icon entirely.
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let id = self.id;
        let dirty = self.has_unsaved_changes();
        let title = div()
            .id(("workspace-tab-title", self.id))
            .test_support()
            .flex()
            .items_center()
            .gap_1()
            .children(self.agent.map(|_| {
                self.agent_icon
                    .clone()
                    .cached(StyleRefinement::default().size(px(16.)))
            }))
            .child(format!("{}{}", self.title, if dirty { " ●" } else { "" }))
            .when(dirty, |title| {
                title.child(
                    Button::new(("close-dirty-tab", self.id))
                        .label("×")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.emit(ContentEvent::RequestClose(this.id));
                            cx.stop_propagation();
                        })),
                )
            });
        if matches!(self.kind, SavedKind::Terminal { .. }) {
            title.into_any_element()
        } else {
            title
                .context_menu(move |menu, _, _| Self::file_menu(menu, weak.clone(), id))
                .into_any_element()
        }
    }
    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        if matches!(self.kind, SavedKind::Terminal { .. }) {
            menu
        } else {
            Self::file_menu(menu, cx.weak_entity(), self.id)
        }
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}
impl Render for WorkspacePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.content {
            Content::Loading => div().p_4().child("Loading…").into_any_element(),
            Content::Error(error) => div()
                .p_4()
                .text_color(rgb(0xffaaaa))
                .child(error.clone())
                .into_any_element(),
            Content::Terminal(terminal) => {
                div().size_full().child(terminal.clone()).into_any_element()
            }
            Content::File {
                editor,
                markdown,
                source,
                ..
            } => {
                let toolbar = div()
                    .flex()
                    .gap_2()
                    .p_1()
                    .child(
                        Button::new("save")
                            .label("Save")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.save(false, cx))),
                    )
                    .children(markdown.as_ref().map(|_| {
                        Button::new("markdown-mode")
                            .label(if *source { "Preview" } else { "Source" })
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Content::File { source, .. } = &mut this.content {
                                    *source = !*source;
                                }
                                cx.notify();
                            }))
                    }));
                let body = if !source && markdown.is_some() {
                    div()
                        .id("markdown-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .p_4()
                        .child(TextView::new(markdown.as_ref().unwrap()))
                        .into_any_element()
                } else {
                    crate::gpui_editor::EditorSurface::new(editor, self.font_size, self.brightness)
                        .into_any_element()
                };
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(toolbar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .text_size(px(self.font_size))
                            .child(body),
                    )
                    .into_any_element()
            }
            Content::Diff(state) => {
                use gpui_kit::base::TestSupportExt;
                let viewport = cx.entity();
                let path = state
                    .read(cx)
                    .files()
                    .first()
                    .map(|file| file.path().to_string())
                    .unwrap_or_else(|| self.path.display().to_string());
                div()
                    .id("workspace-diff")
                    .test_support()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(30.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .w_full()
                            .overflow_hidden()
                            .bg(rgb(0x1d2733))
                            .child(
                                div()
                                    .id("diff-original-heading")
                                    .test_support()
                                    .role(gpui_kit::accesskit::Role::Label)
                                    .aria_label(format!("{path} (HEAD)"))
                                    .flex_1()
                                    .min_w_0()
                                    .px_2()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .child(format!("{path} (HEAD)")),
                            )
                            .child(
                                div()
                                    .id("diff-modified-heading")
                                    .test_support()
                                    .role(gpui_kit::accesskit::Role::Label)
                                    .aria_label(format!("{path} (Working Tree)"))
                                    .flex_1()
                                    .min_w_0()
                                    .px_2()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .child(format!("{path} (Working Tree)")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .overflow_hidden()
                            .on_prepaint(move |bounds, window, cx| {
                                let changed = viewport.update(cx, |this, _| {
                                    if this.diff_viewport == bounds.size {
                                        return false;
                                    }
                                    this.diff_viewport = bounds.size;
                                    true
                                });
                                if changed {
                                    // The Kit measures its viewport during paint.
                                    // Re-render after that frame so cached dock
                                    // content uses the new width when wrapping.
                                    let viewport = viewport.clone();
                                    window.defer(cx, move |_, cx| {
                                        viewport.update(cx, |_, cx| cx.notify())
                                    });
                                }
                            })
                            .child(Diff::new(state).soft_wrap(true).header_visible(false)),
                    )
                    .into_any_element()
            }
            Content::Preview {
                image,
                page,
                count,
                zoom,
                width,
                height,
                text,
                ..
            } => {
                let zoom = *zoom;
                let selection = self
                    .pdf_selection
                    .map(|(a, b)| text.selection_rects(a, b))
                    .unwrap_or_default();
                let measure = cx.entity();
                let viewport = cx.entity();
                let is_pdf = *count > 0;
                let display_width = *width * if is_pdf { 1. } else { zoom };
                let display_height = *height * if is_pdf { 1. } else { zoom };
                let fit_scale = if self.preview_fit
                    && self.preview_viewport.width > px(0.)
                    && self.preview_viewport.height > px(0.)
                {
                    (f32::from(self.preview_viewport.width) / display_width)
                        .min(f32::from(self.preview_viewport.height) / display_height)
                        .min(1.)
                        .max(0.001)
                } else {
                    1.
                };
                self.preview_scale = zoom * fit_scale;
                let selection_scale = self.preview_scale;
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .p_1()
                            .when(is_pdf, |bar| {
                                bar.child(
                                    Button::new("previous-page")
                                        .label("Previous")
                                        .small()
                                        .on_click(
                                            cx.listener(move |this, _, _, _| this.page(-1, zoom)),
                                        ),
                                )
                                .child(format!("{} / {}", page + 1, count))
                                .child(
                                    Button::new("next-page").label("Next").small().on_click(
                                        cx.listener(move |this, _, _, _| this.page(1, zoom)),
                                    ),
                                )
                            })
                            .child(Button::new("preview-fit").label("Fit").small().on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.preview_fit = true;
                                    this.pdf_selection = None;
                                    cx.notify();
                                }),
                            ))
                            .child(Button::new("zoom-out").label("−").small().on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if is_pdf {
                                        this.page(0, (zoom / 1.25).max(0.25));
                                    } else if let Content::Preview { zoom, .. } = &mut this.content
                                    {
                                        *zoom = (*zoom / 1.25).max(0.1);
                                        this.preview_fit = false;
                                        cx.notify();
                                    }
                                }),
                            ))
                            .child(Button::new("zoom-in").label("+").small().on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if is_pdf {
                                        this.page(0, (zoom * 1.25).min(4.));
                                    } else if let Content::Preview { zoom, .. } = &mut this.content
                                    {
                                        *zoom = (*zoom * 1.25).min(8.);
                                        this.preview_fit = false;
                                        cx.notify();
                                    }
                                }),
                            )),
                    )
                    .child(
                        div()
                            .id("preview-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_scroll()
                            .on_prepaint(move |bounds, window, cx| {
                                let changed = viewport.update(cx, |this, _| {
                                    if this.preview_viewport != bounds.size {
                                        this.preview_viewport = bounds.size;
                                        true
                                    } else {
                                        false
                                    }
                                });
                                if changed {
                                    let viewport = viewport.clone();
                                    window.defer(cx, move |_, cx| {
                                        viewport.update(cx, |_, cx| cx.notify())
                                    });
                                }
                            })
                            .child(
                                div()
                                    .id("preview-image")
                                    .relative()
                                    .w(px(display_width * fit_scale))
                                    .h(px(display_height * fit_scale))
                                    .on_prepaint(move |bounds, _, cx| {
                                        measure.update(cx, |this, _| this.preview_bounds = bounds)
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                            this.focus.focus(window, cx);
                                            this.selecting_pdf = true;
                                            this.select_pdf(event.position, true, cx);
                                        }),
                                    )
                                    .on_mouse_move(cx.listener(
                                        |this, event: &MouseMoveEvent, _, cx| {
                                            if this.selecting_pdf {
                                                this.select_pdf(event.position, false, cx);
                                            }
                                        },
                                    ))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, _| this.selecting_pdf = false),
                                    )
                                    .child(img(image.clone()).size_full())
                                    .children(selection.into_iter().map(
                                        move |(x, y, width, height)| {
                                            div()
                                                .absolute()
                                                .left(px(x * selection_scale))
                                                .top(px(y * selection_scale))
                                                .w(px(width * selection_scale))
                                                .h(px(height * selection_scale))
                                                .bg(rgba(0x4090ff60))
                                        },
                                    )),
                            ),
                    )
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.control
                    && event.keystroke.key == "c"
                    && matches!(this.content, Content::Preview { .. })
                {
                    this.copy_pdf(cx);
                    cx.stop_propagation();
                }
            }))
            .when(self.conflict, |view| {
                view.child(
                    div()
                        .flex()
                        .gap_2()
                        .bg(rgb(0x664400))
                        .p_2()
                        .child("File changed on disk")
                        .child(
                            Button::new("overwrite")
                                .label("Overwrite")
                                .on_click(cx.listener(|this, _, _, cx| this.save(true, cx))),
                        )
                        .child(
                            Button::new("reload")
                                .label("Reload")
                                .on_click(cx.listener(|this, _, _, _| this.reload())),
                        ),
                )
            })
            .child(div().flex_1().min_h_0().child(content))
    }
}
