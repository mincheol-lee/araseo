//! Docked code-navigation sidebar; filesystem and LSP work stay on workers.
use crate::{
    background::Background,
    code_navigation::{self as nav, Language, Query, Request, Response, Row, Service, Snapshot},
    lsp_transport::Cancellation,
    workspace::Workspace,
};
use gpui_kit::component::{
    Sizable,
    button::{Button, ButtonVariants},
    input::{DefinitionProvider, Input, InputEvent, InputState, Rope},
};
use gpui_kit::{prelude::*, *};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

actions!(
    araseo_navigation,
    [
        SearchFunctions,
        FindReferences,
        FindDefinitions,
        ShowCallers,
        NavigateBack,
        NavigateForward
    ]
);
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "f12",
            gpui_kit::component::input::GoToDefinition,
            Some("Input && mode == editor"),
        ),
        KeyBinding::new("ctrl-t", SearchFunctions, Some("Araseo")),
        KeyBinding::new(
            "alt-f12",
            FindDefinitions,
            Some("Araseo > (Input && mode == editor)"),
        ),
        KeyBinding::new(
            "shift-f12",
            FindReferences,
            Some("Araseo > (Input && mode == editor)"),
        ),
        KeyBinding::new(
            "alt-shift-h",
            ShowCallers,
            Some("Araseo > (Input && mode == editor)"),
        ),
        KeyBinding::new("alt-left", NavigateBack, Some("Araseo")),
        KeyBinding::new("alt-right", NavigateForward, Some("Araseo")),
    ]);
}
#[derive(Clone)]
pub enum NavigationEvent {
    Jump(nav::Location),
    Close,
    Refresh,
}
impl EventEmitter<NavigationEvent> for Navigator {}

#[derive(Clone)]
pub struct Seed {
    pub workspace: Workspace,
    pub path: PathBuf,
    pub directory: bool,
}
#[derive(Clone)]
enum Entry {
    File {
        path: PathBuf,
        first: usize,
        count: usize,
    },
    Result(usize),
}
pub struct Navigator {
    focus: FocusHandle,
    input: Entity<InputState>,
    pub project: Option<nav::Project>,
    seed: Seed,
    projects: Vec<Seed>,
    language: Language,
    query: Query,
    snapshots: Vec<Snapshot>,
    pub snapshot_source: Option<Rc<dyn Fn(&App) -> Vec<Snapshot>>>,
    service: Service,
    worker: Background<(nav::Project, Result<Response, String>)>,
    cancel: Cancellation,
    due: Option<Instant>,
    rows: Vec<Row>,
    selected: usize,
    entries: Vec<Entry>,
    collapsed: std::collections::HashSet<PathBuf>,
    busy: bool,
    note: String,
    expanded: std::collections::HashSet<usize>,
    expand_at: Option<usize>,
    scroll: UniformListScrollHandle,
    _input: Subscription,
    _poll: Task<()>,
}
impl Navigator {
    pub fn new(
        seed: Seed,
        projects: Vec<Seed>,
        language: Language,
        query: Query,
        snapshots: Vec<Snapshot>,
        service: Service,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search project functions…"));
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && matches!(this.query, Query::Symbols(_)) {
                this.cancel.cancel();
                this.worker.cancel();
                this.rows.clear();
                this.entries.clear();
                this.expanded.clear();
                this.selected = 0;
                this.due = Some(Instant::now() + Duration::from_millis(220));
                this.busy = true;
                cx.notify();
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this.update_in(cx, |this, _, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        let focus = cx.focus_handle();
        if matches!(query, Query::Symbols(_)) {
            input.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            focus.focus(window, cx);
        }
        let mut this = Self {
            focus,
            input,
            project: None,
            seed,
            projects,
            language,
            query,
            snapshots,
            snapshot_source: None,
            service,
            worker: Background::default(),
            cancel: Cancellation::default(),
            due: None,
            rows: vec![],
            selected: 0,
            entries: vec![],
            collapsed: Default::default(),
            busy: true,
            note: "Starting language server…".into(),
            expanded: Default::default(),
            expand_at: None,
            scroll: UniformListScrollHandle::new(),
            _input: subscription,
            _poll: task,
        };
        this.submit(cx);
        this
    }
    pub fn cancel(&mut self) {
        self.cancel.cancel();
        self.worker.cancel();
    }
    pub fn refresh(&mut self, snapshots: Vec<Snapshot>, cx: &mut Context<Self>) {
        self.snapshots = snapshots;
        self.expand_at = None;
        self.submit(cx);
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        self.cancel();
        self.cancel = Cancellation::default();
        self.due = None;
        self.busy = true;
        let mut query = self
            .expand_at
            .and_then(|i| self.rows.get(i))
            .and_then(|r| {
                r.item
                    .clone()
                    .map(|item| Query::Incoming(item, r.depth + 1))
            })
            .unwrap_or_else(|| self.query.clone());
        if matches!(query, Query::Symbols(_)) {
            query = Query::Symbols(self.input.read(cx).value().to_string());
        }
        let seed = self.seed.clone();
        let language = self.language;
        let snapshots = self
            .snapshot_source
            .as_ref()
            .map(|get| get(cx))
            .unwrap_or_else(|| self.snapshots.clone());
        let service = self.service.clone();
        let cancel = self.cancel.clone();
        self.worker.request(move || {
            let project = nav::project_for(&seed.workspace, &seed.path, seed.directory);
            match project {
                Ok(project) => {
                    let result = service.execute(
                        Request {
                            project: project.clone(),
                            language,
                            query,
                            snapshots,
                        },
                        &cancel,
                    );
                    (project, result)
                }
                Err(error) => (
                    nav::Project {
                        workspace: seed.workspace,
                        root: seed.path,
                    },
                    Err(error),
                ),
            }
        });
        cx.notify();
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.due.is_some_and(|due| Instant::now() >= due) {
            self.submit(cx);
        }
        if let Some((project, result)) = self.worker.poll() {
            self.busy = false;
            self.project = Some(project);
            match result {
                Ok(response) => {
                    self.note = response.note;
                    if let Some(index) = self.expand_at.take() {
                        if response.rows.is_empty() {
                            self.note = "No further callers".into();
                        }
                        let available = nav::LIMIT.saturating_sub(self.rows.len());
                        let added = response
                            .rows
                            .into_iter()
                            .take(available)
                            .collect::<Vec<_>>();
                        let count = added.len();
                        self.expanded = self
                            .expanded
                            .iter()
                            .map(|i| if *i > index { *i + count } else { *i })
                            .collect();
                        self.rows.splice(index + 1..index + 1, added);
                        self.expanded.insert(index);
                    } else {
                        self.replace_rows(response.rows);
                    }
                    self.rebuild_entries();
                    if self.rows.is_empty() {
                        self.note = format!("No results. {}", self.note);
                    }
                }
                Err(error) => {
                    self.expand_at = None;
                    self.note = error;
                }
            }
            cx.notify();
        }
        if let Some(error) = self.worker.take_error() {
            self.busy = false;
            self.note = error;
            cx.notify();
        }
    }
    fn jump(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index;
        cx.notify();
        if let Some(row) = self.rows.get(index) {
            cx.emit(NavigationEvent::Jump(row.location.clone()));
        }
    }
    fn expand(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.expanded.contains(&index) {
            let depth = self.rows[index].depth;
            let mut end = index + 1;
            while end < self.rows.len() && self.rows[end].depth > depth {
                end += 1;
            }
            self.rows.drain(index + 1..end);
            self.expanded = self
                .expanded
                .iter()
                .filter_map(|i| {
                    if *i < index {
                        Some(*i)
                    } else if *i >= end {
                        Some(*i - (end - index - 1))
                    } else {
                        None
                    }
                })
                .collect();
            self.selected = index;
            self.rebuild_entries();
            cx.notify();
            return;
        }
        if self.rows.get(index).is_some_and(|row| row.item.is_some()) {
            self.expand_at = Some(index);
            self.submit(cx);
        }
    }
    fn replace_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.selected = 0;
        self.expanded = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                self.rows
                    .get(i + 1)
                    .filter(|next| next.depth > row.depth)
                    .map(|_| i)
            })
            .collect();
        self.rebuild_entries();
    }
    fn rebuild_entries(&mut self) {
        self.entries.clear();
        let tree = self.rows.iter().any(|r| r.item.is_some());
        if tree {
            self.entries.extend((0..self.rows.len()).map(Entry::Result));
            return;
        }
        let mut start = 0;
        while start < self.rows.len() {
            let path = self.rows[start].location.path.clone();
            let mut end = start + 1;
            while end < self.rows.len() && self.rows[end].location.path == path {
                end += 1;
            }
            self.entries.push(Entry::File {
                path: path.clone(),
                first: start,
                count: end - start,
            });
            if !self.collapsed.contains(&path) {
                self.entries.extend((start..end).map(Entry::Result));
            }
            start = end;
        }
    }
    fn toggle_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
        self.rebuild_entries();
        if !self
            .entries
            .iter()
            .any(|e| matches!(e,Entry::Result(i) if *i==self.selected))
        {
            if let Some(index) = self.entries.iter().find_map(|e| match e {
                Entry::Result(i) => Some(*i),
                _ => None,
            }) {
                self.selected = index;
            }
        }
        cx.notify();
    }
    fn select(&mut self, delta: isize, cx: &mut Context<Self>) {
        let visible = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Result(i) => Some(*i),
                _ => None,
            })
            .collect::<Vec<_>>();
        if visible.is_empty() {
            return;
        }
        let current = visible
            .iter()
            .position(|i| *i == self.selected)
            .unwrap_or(0);
        self.selected =
            visible[(current as isize + delta).clamp(0, visible.len() as isize - 1) as usize];
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| matches!(entry, Entry::Result(i) if *i == self.selected))
        {
            self.scroll.scroll_to_item(index, ScrollStrategy::Top);
        }
        cx.notify();
    }
    fn relative_path(&self, path: &std::path::Path) -> String {
        path.strip_prefix(
            self.project
                .as_ref()
                .map(|p| p.root.as_path())
                .unwrap_or(&self.seed.path),
        )
        .unwrap_or(path)
        .display()
        .to_string()
    }
    fn result_row(&self, entry: usize, cx: &mut Context<Self>) -> AnyElement {
        if let Entry::File { path, first, count } = &self.entries[entry] {
            let path = path.clone();
            let collapsed = self.collapsed.contains(&path);
            let first = *first;
            return div()
                .id(("nav-file", entry))
                .w_full()
                .h(px(30.))
                .flex()
                .items_center()
                .min_w_0()
                .px_1()
                .child(
                    Button::new(("nav-file-expand", entry))
                        .label(if collapsed { "▸" } else { "▾" })
                        .small()
                        .ghost()
                        .accessibility_label(format!("Toggle {}", self.relative_path(&path)))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_file(path.clone(), cx)),
                        ),
                )
                .child(
                    div()
                        .id(("nav-file-open", entry))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Button)
                        .aria_label(format!(
                            "Open {}",
                            self.relative_path(match &self.entries[entry] {
                                Entry::File { path, .. } => path,
                                _ => unreachable!(),
                            })
                        ))
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .cursor_pointer()
                        .child(self.relative_path(match &self.entries[entry] {
                            Entry::File { path, .. } => path,
                            _ => unreachable!(),
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.jump(first, cx))),
                )
                .child(div().px_1().text_size(px(11.)).child(count.to_string()))
                .into_any_element();
        }
        let Entry::Result(index) = self.entries[entry] else {
            unreachable!()
        };
        let row = &self.rows[index];
        let path = self.relative_path(&row.location.path);
        let tree = row.item.is_some();
        let mut text = div().flex().min_w_0().overflow_hidden().gap_1();
        if let Some(snippet) = &row.snippet {
            let span = &snippet.highlight;
            text = text
                .child(
                    div()
                        .text_color(rgb(0x9caec4))
                        .child(format!("{}", row.location.range.start.line + 1)),
                )
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .overflow_hidden()
                        .child(snippet.text[..span.start].to_string())
                        .child(
                            div()
                                .bg(rgb(0x365573))
                                .child(snippet.text[span.clone()].to_string()),
                        )
                        .child(snippet.text[span.end..].to_string()),
                );
        } else {
            text = text
                .child(div().text_ellipsis().child(row.name.clone()))
                .when(tree, |view| {
                    view.child(
                        div()
                            .text_color(rgb(0x9caec4))
                            .text_ellipsis()
                            .child(format!("({path})")),
                    )
                });
        }
        div()
            .id(("nav-result", index))
            .w_full()
            .test_support()
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(format!(
                "{} · {}:{}",
                row.name,
                path,
                row.location.range.start.line + 1
            ))
            .h(px(30.))
            .flex()
            .items_center()
            .min_w_0()
            .pl(px(8. + if tree { row.depth as f32 * 14. } else { 14. }))
            .pr_1()
            .cursor_pointer()
            .when(index == self.selected, |view| view.bg(rgb(0x344b65)))
            .hover(|view| view.bg(rgb(0x30445c)))
            .children(row.item.as_ref().map(|_| {
                Button::new(("nav-expand", index))
                    .label(if self.expanded.contains(&index) {
                        "▾"
                    } else {
                        "▸"
                    })
                    .small()
                    .ghost()
                    .accessibility_label(format!("Expand {}", row.name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.expand(index, cx);
                        cx.stop_propagation();
                    }))
            }))
            .child(text)
            .on_click(cx.listener(move |this, _, _, cx| this.jump(index, cx)))
            .into_any_element()
    }
    #[cfg(feature = "ui-tests")]
    pub fn visible_results_for_test(&self) -> Vec<usize> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Result(i) => Some(*i),
                _ => None,
            })
            .collect()
    }
    #[cfg(feature = "ui-tests")]
    pub fn toggle_file_for_test(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.toggle_file(path, cx);
    }
    #[cfg(feature = "ui-tests")]
    pub fn query_for_test(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }
    #[cfg(feature = "ui-tests")]
    pub fn set_results_for_test(&mut self, rows: Vec<Row>) {
        self.cancel();
        self.busy = false;
        self.replace_rows(rows);
    }
    #[cfg(feature = "ui-tests")]
    pub fn collapse_for_test(&mut self, index: usize, cx: &mut Context<Self>) {
        self.expand(index, cx);
    }
}
impl Drop for Navigator {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Render for Navigator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
        let searching = matches!(self.query, Query::Symbols(_));
        let title = if searching {
            "Project functions"
        } else if matches!(self.query, Query::Definition(..)) {
            "Definitions"
        } else {
            if self.rows.iter().any(|r| r.item.is_some())
                || matches!(self.query, Query::Callers(..)) && !self.note.contains("unsupported")
            {
                "Call Hierarchy: Callers Of"
            } else {
                "References"
            }
        };
        let root = self
            .project
            .as_ref()
            .map(|p| p.label())
            .unwrap_or_else(|| self.seed.path.display().to_string());
        let projects = self.projects.clone();
        let weak = cx.weak_entity();
        let project_picker = Button::new("nav-project-picker")
            .label("Change project ▾")
            .small()
            .ghost()
            .dropdown_menu(move |mut menu, _, _| {
                for seed in &projects {
                    let seed = seed.clone();
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(seed.path.display().to_string()).on_click(
                            move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.seed = seed.clone();
                                    this.project = None;
                                    this.rows.clear();
                                    this.expanded.clear();
                                    this.expand_at = None;
                                    this.query = Query::Symbols(String::new());
                                    this.submit(cx);
                                });
                            },
                        ),
                    );
                }
                menu
            });
        let weak = cx.weak_entity();
        let language_picker = Button::new("nav-language-picker")
            .label(format!("{} ▾", self.language.name()))
            .small()
            .ghost()
            .dropdown_menu(move |mut menu, _, _| {
                for language in Language::ALL {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new(language.name()).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.language = language;
                                this.rows.clear();
                                this.expanded.clear();
                                this.expand_at = None;
                                this.query = Query::Symbols(String::new());
                                this.submit(cx);
                            });
                        },
                    ));
                }
                menu
            });
        let entity = cx.entity();
        let count = self.entries.len();
        let list = uniform_list("nav-results", count, move |range, _, cx| {
            entity.update(cx, |this, cx| {
                range.map(|i| this.result_row(i, cx)).collect::<Vec<_>>()
            })
        })
        .flex_1()
        .min_h_0()
        .track_scroll(&self.scroll);
        let files = self
            .rows
            .iter()
            .map(|r| &r.location.path)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let _ = window;
        div()
            .id("code-navigation")
            .test_support()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(0x181f29))
            .text_size(px(12.))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                // Only this sidebar's focus path handles these keys; editors and PTYs remain independent.
                match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.emit(NavigationEvent::Close);
                    }
                    "up" => this.select(-1, cx),
                    "down" => this.select(1, cx),
                    "enter" => {
                        if this
                            .entries
                            .iter()
                            .any(|e| matches!(e,Entry::Result(i) if *i==this.selected))
                        {
                            this.jump(this.selected, cx);
                        }
                    }
                    "right" if !matches!(this.query, Query::Symbols(_)) => {
                        if this.expanded.contains(&this.selected) {
                            if this
                                .rows
                                .get(this.selected + 1)
                                .is_some_and(|next| next.depth > this.rows[this.selected].depth)
                            {
                                this.select(1, cx);
                            }
                        } else {
                            this.expand(this.selected, cx);
                        }
                    }
                    "left" if !matches!(this.query, Query::Symbols(_)) => {
                        if this.expanded.contains(&this.selected) {
                            this.expand(this.selected, cx);
                        } else if let Some(row) = this.rows.get(this.selected) {
                            if let Some(parent) = (0..this.selected)
                                .rev()
                                .find(|i| this.rows[*i].depth < row.depth)
                            {
                                this.selected = parent;
                                this.scroll.scroll_to_item(parent, ScrollStrategy::Top);
                                cx.notify();
                            }
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .px_1()
                    .h(px(30.))
                    .flex_shrink_0()
                    .child(div().flex_1().min_w_0().text_ellipsis().child(title))
                    .child(
                        Button::new("nav-retry")
                            .label("↻")
                            .accessibility_label("Refresh navigation")
                            .tooltip("Refresh results")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(NavigationEvent::Refresh))),
                    )
                    .child(
                        Button::new("nav-close")
                            .label("×")
                            .accessibility_label("Close navigation")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(NavigationEvent::Close))),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .text_size(px(11.))
                    .text_color(rgb(0x9ab9db))
                    .text_ellipsis()
                    .child(format!("Project: {root}")),
            )
            .when(searching, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .child(project_picker)
                        .child(language_picker),
                )
                .child(
                    div()
                        .id("nav-query")
                        .test_support()
                        .px_1()
                        .child(Input::new(&self.input).aria_label("Search project functions")),
                )
            })
            .child(div().px_2().py_1().text_color(rgb(0x9caec4)).child(format!(
                "{} {} in {} {}",
                self.rows.len(),
                if self.rows.len() == 1 {
                    "result"
                } else {
                    "results"
                },
                files,
                if files == 1 { "file" } else { "files" }
            )))
            .when(self.busy || !self.note.is_empty(), |view| {
                view.child(
                    div()
                        .id("nav-status")
                        .px_2()
                        .max_h(px(110.))
                        .overflow_y_scroll()
                        .text_size(px(11.))
                        .child(if self.busy {
                            "Analyzing… (first use may take longer)".into()
                        } else {
                            self.note.clone()
                        }),
                )
            })
            .child(list)
            .when(self.rows.is_empty() && !self.busy, |view| {
                view.child(
                    Button::new("nav-install")
                        .label("Copy WSL install command")
                        .small()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                this.language.install().into(),
                            ))
                        })),
                )
            })
    }
}

/// Use the Kit's F12 and Ctrl+click behavior instead of reimplementing its
/// cursor hit testing. Its show-document hook delegates actual opening to Araseo.
pub struct Definitions {
    pub service: Service,
    pub workspace: Workspace,
    pub path: PathBuf,
    pub snapshots: Rc<dyn Fn(&App) -> Vec<Snapshot>>,
    pub error: Rc<dyn Fn(String, &mut App)>,
    pub cancel: RefCell<Cancellation>,
    pub origin: Rc<Cell<nav::Position>>,
}
struct CancelOnDrop(Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl Drop for Definitions {
    fn drop(&mut self) {
        self.cancel.borrow().cancel();
    }
}
impl DefinitionProvider for Definitions {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<lsp_types::LocationLink>>> {
        self.cancel.borrow().cancel();
        let cancel = Cancellation::default();
        *self.cancel.borrow_mut() = cancel.clone();
        let text = text.to_string();
        let pos = nav::position(&text, offset);
        let path = self.path.clone();
        let mut snapshots = (self.snapshots)(cx);
        snapshots.retain(|s| s.path != path);
        snapshots.push(Snapshot {
            path: path.clone(),
            text,
        });
        let service = self.service.clone();
        let workspace = self.workspace.clone();
        let error = self.error.clone();
        let origin = self.origin.clone();
        let guard = CancelOnDrop(cancel.clone());
        let task = cx.background_executor().spawn(async move {
            let language = Language::for_path(&path)
                .ok_or_else(|| "No language server configured for this file type".to_string())?;
            let project = nav::project_for(&workspace, &path, false)?;
            service.execute(
                Request {
                    project,
                    language,
                    query: Query::Definition(pos, path),
                    snapshots,
                },
                &cancel,
            )
        });
        cx.spawn(async move |cx| {
            let _guard = guard;
            match task.await {
                Ok(response) => {
                    origin.set(pos);
                    Ok(response
                        .rows
                        .into_iter()
                        .filter_map(|row| {
                            let range = lsp_types::Range {
                                start: lsp_types::Position::new(
                                    row.location.range.start.line,
                                    row.location.range.start.character,
                                ),
                                end: lsp_types::Position::new(
                                    row.location.range.end.line,
                                    row.location.range.end.character,
                                ),
                            };
                            Some(lsp_types::LocationLink {
                                origin_selection_range: None,
                                target_uri: nav::uri(&row.location.path).parse().ok()?,
                                target_range: range,
                                target_selection_range: range,
                            })
                        })
                        .collect())
                }
                Err(message) => {
                    let display = message.clone();
                    if !display.contains("cancelled") {
                        let _ = cx.update(|cx| error(display, cx));
                    }
                    Err(anyhow::anyhow!(message))
                }
            }
        })
    }
}
