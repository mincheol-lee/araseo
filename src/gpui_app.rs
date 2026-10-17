//! GPUI workbench. Terminal output invalidates only its terminal entity;
//! filesystem and Git work stay on the existing background workers.
use crate::{
    appearance::FontSizes,
    background::Background,
    document::Document,
    git, git_view,
    gpui_panel::{Content, ContentEvent, Opened, WorkspacePanel},
    session::{SavedKind, SavedTab, Session},
    tabs::{LayoutSpec, TabGroups, TabId},
    tree::{self, FlatNode, GitStatus},
    workspace::{self, Workspace},
    workspace_history, wsl_diagnostics,
};
use gpui_kit::component::{
    Icon, IconName, Sizable, Theme, ThemeMode,
    button::{Button, ButtonVariants},
    dock::{
        self, DockArea, DockAreaState, DockLayout, DockPlacement, DockSkin, PanelId, PanelInfo,
    },
    input::{Input, InputEvent, InputState},
    menu::ContextMenuExt,
};
use gpui_kit::{prelude::*, *};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

actions!(
    araseo,
    [
        Save,
        NewTerminal,
        NextTab,
        PreviousTab,
        CloseTab,
        Diagnostics,
        Settings
    ]
);
#[derive(Clone)]
pub(crate) struct DragPath(pub(crate) PathBuf);
impl Render for DragPath {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .bg(rgb(0x303844))
            .child(self.0.display().to_string())
    }
}
#[derive(Clone)]
enum DialogAction {
    Create(PathBuf, bool),
    Rename(PathBuf),
    Delete(PathBuf),
    Discard(PathBuf),
    AddFolder,
    Kill(wsl_diagnostics::ProcessTarget, bool),
    Close(TabId),
}
struct Dialog {
    title: String,
    action: DialogAction,
    input: Option<Entity<InputState>>,
    _subscription: Option<Subscription>,
}
type Catalog = Rc<RefCell<HashMap<TabId, Entity<WorkspacePanel>>>>;
struct TreeResult {
    nodes: Vec<FlatNode>,
}
enum ActionResult {
    EntryCreated {
        path: PathBuf,
        parent: PathBuf,
        directory: bool,
    },
    TreeChanged(Option<(PathBuf, PathBuf)>),
    Folder(Workspace),
    Message(String),
    ProcessStopped(String),
}

pub struct Workbench {
    workspace: Workspace,
    added: Vec<Workspace>,
    expanded: HashSet<PathBuf>,
    statuses: HashMap<PathBuf, GitStatus>,
    repositories: HashSet<PathBuf>,
    git_groups: Vec<git_view::Group>,
    git_rows: Vec<git_view::Row>,
    collapsed_git: HashSet<PathBuf>,
    tree: Vec<FlatNode>,
    tree_selection: Option<PathBuf>,
    tree_loader: Background<Result<TreeResult, String>>,
    tree_pending: bool,
    monitor_loader: Background<Vec<(PathBuf, Result<git::StatusMonitor, String>)>>,
    monitors: Vec<(PathBuf, git::StatusMonitor)>,
    action_loader: Background<Result<ActionResult, String>>,
    diagnostics_loader: Background<wsl_diagnostics::Report>,
    docker_loader: Background<Result<Vec<wsl_diagnostics::DockerUsage>, String>>,
    agent_loader: Background<Vec<(TabId, Option<crate::terminal::AgentStatus>)>>,
    agent_tick: u16,
    dock: Entity<DockArea>,
    _skin: Rc<DockSkin>,
    caption_node: Rc<Cell<Option<dock::NodeId>>>,
    panels: Catalog,
    subscriptions: HashMap<TabId, Subscription>,
    geometry: (u32, u32, bool),
    next_id: TabId,
    next_terminal: u32,
    focused: Option<TabId>,
    focus: FocusHandle,
    fonts: FontSizes,
    sidebar_git: bool,
    sidebar_width: f32,
    resizing_sidebar: bool,
    status: String,
    dialog: Option<Dialog>,
    settings_open: bool,
    report: Option<wsl_diagnostics::Report>,
    diagnostics_open: bool,
    save_at: Option<Instant>,
    session_path: Option<PathBuf>,
    session_writer: crate::session::SessionWriter,
    close_task: Option<Task<()>>,
    closing: bool,
    legacy_groups: TabGroups,
    restore_queue: VecDeque<SavedTab>,
    restoring: HashSet<TabId>,
    restore_focus: Option<TabId>,
    _poll: Task<()>,
    _dock_subscription: Subscription,
    _quit: Subscription,
    _activation: Subscription,
    last_poll: Instant,
    last_heartbeat: Instant,
}
impl Workbench {
    pub fn new(
        workspace: Workspace,
        saved: Option<Session>,
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let saved = saved.map(|mut saved| {
            let ids = saved.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
            let available = ids.iter().copied().collect::<HashSet<_>>();
            saved.groups =
                TabGroups::restore(saved.groups, &ids, &available).unwrap_or_else(|| {
                    let mut groups = TabGroups::default();
                    for id in ids {
                        groups.add(id, 0);
                    }
                    groups
                });
            saved
        });
        let fonts = crate::appearance::settings_path()
            .map(|path| FontSizes::load(&path))
            .unwrap_or_default();
        let caption_node = Rc::new(Cell::new(None));
        let mut skin = None;
        let new_terminal_root = cx.weak_entity();
        let close_root = cx.weak_entity();
        let dock = cx.new(|cx| {
            let dock_skin = DockSkin::new(cx);
            skin = Some(dock_skin.clone());
            DockArea::new("araseo-workbench", Some(1), window, cx).with_renderer(Rc::new(
                crate::gpui_chrome::CompactDock::new(
                    dock_skin,
                    caption_node.clone(),
                    Rc::new(move |window, cx| {
                        let _ = new_terminal_root.update(cx, |this: &mut Self, cx| {
                            this.new_terminal(this.workspace.linux_root.clone(), window, cx)
                        });
                    }),
                    Rc::new(move |window, cx| {
                        let _ = close_root.update(cx, |this: &mut Self, cx| {
                            if this.close(window, cx) {
                                window.remove_window();
                            }
                        });
                    }),
                ),
            ))
        });
        let skin = skin.expect("dock skin constructed");
        skin.set_close_button_visible(true, cx);
        let panels: Catalog = Rc::new(RefCell::new(HashMap::new()));
        let registry = panels.clone();
        dock::register_panel(cx, "AraseoTab", move |context, _, _| {
            let PanelInfo::Panel(value) = context.info() else {
                panic!("invalid Araseo panel state")
            };
            let id = value
                .get("id")
                .and_then(|id| id.as_u64())
                .expect("validated panel ID") as u32;
            let panel = registry
                .borrow()
                .get(&id)
                .expect("validated session panel")
                .clone();
            dock::panel_handle(panel)
        });
        let _dock_subscription =
            cx.subscribe_in(&dock, window, |this, _, _: &dock::DockEvent, _, cx| {
                this.schedule_save();
                cx.notify();
            });
        let _quit = cx.on_app_quit(|this, cx| {
            let snapshot = this.snapshot(cx);
            let path = this.session_path.clone();
            let writer = this.session_writer.clone();
            let generation = writer.reserve();
            cx.background_executor().spawn(async move {
                if let (Some(snapshot), Some(path)) = (snapshot, path) {
                    if let Err(error) = writer.save(generation, &snapshot, &path) {
                        crate::diagnostics_log::event(&format!("session save failed: {error}"));
                    }
                }
            })
        });
        let activation = cx.observe_window_activation(window, |_, window, _| {
            crate::diagnostics_log::event(if window.is_window_active() {
                "window focused"
            } else {
                "window unfocused"
            })
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| this.poll(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| this.close(window, cx))
                .unwrap_or(true)
        });
        let session_path = crate::session::settings_path(&workspace.distro, &workspace.linux_root);
        let history = workspace_history::settings_path(&workspace.distro, &workspace.linux_root)
            .map(|path| workspace_history::load(&path, &workspace.distro, &workspace.linux_root))
            .unwrap_or_default();
        let mut this = Self {
            workspace,
            added: vec![],
            expanded: HashSet::new(),
            statuses: HashMap::new(),
            repositories: HashSet::new(),
            git_groups: Vec::new(),
            git_rows: Vec::new(),
            collapsed_git: HashSet::new(),
            tree: vec![],
            tree_selection: None,
            tree_loader: Background::default(),
            tree_pending: false,
            monitor_loader: Background::default(),
            monitors: vec![],
            action_loader: Background::default(),
            diagnostics_loader: Background::default(),
            docker_loader: Background::default(),
            agent_loader: Background::default(),
            agent_tick: 0,
            dock,
            _skin: skin,
            caption_node,
            panels,
            subscriptions: HashMap::new(),
            geometry: window_geometry(window),
            next_id: 1,
            next_terminal: 1,
            focused: None,
            focus: cx.focus_handle(),
            fonts,
            sidebar_git: saved.as_ref().is_some_and(|s| s.sidebar_view == 1),
            sidebar_width: saved.as_ref().map(|s| s.sidebar_width).unwrap_or(250.),
            resizing_sidebar: false,
            status: "Ready".into(),
            dialog: None,
            settings_open: false,
            report: None,
            diagnostics_open: false,
            save_at: None,
            session_path,
            session_writer: crate::session::SessionWriter::default(),
            close_task: None,
            closing: false,
            legacy_groups: saved.as_ref().map(|s| s.groups.clone()).unwrap_or_default(),
            restore_queue: VecDeque::new(),
            restoring: HashSet::new(),
            restore_focus: None,
            _poll: task,
            _dock_subscription,
            _quit,
            _activation: activation,
            last_poll: Instant::now(),
            last_heartbeat: Instant::now(),
        };
        for path in history {
            if let Ok(workspace) = Workspace::new(this.workspace.distro.clone(), path) {
                this.added.push(workspace);
            }
        }
        this.restart_monitors();
        this.refresh_tree();
        if let Some(saved) = saved {
            let ids: HashSet<_> = saved.tabs.iter().map(|tab| tab.id).collect();
            for tab in &saved.tabs {
                this.create_panel(tab.clone(), cx);
            }
            let zoom_tab = if saved.gpui_layout.is_some() {
                saved.maximized_group.and_then(|id| u32::try_from(id).ok())
            } else {
                saved
                    .maximized_group
                    .and_then(|group| saved.groups.active(group))
            };
            let layout = saved
                .gpui_layout
                .and_then(|value| serde_json::from_value::<DockAreaState>(value).ok())
                .filter(|state| valid_layout(state, &ids));
            if let Some(layout) = layout {
                let _ = this
                    .dock
                    .update(cx, |dock, cx| dock.load(layout, window, cx));
            } else {
                let layout = legacy_layout(
                    &saved.groups.layout_spec(),
                    &saved.groups,
                    &this.panels.borrow(),
                    cx,
                );
                this.dock
                    .update(cx, |dock, cx| dock.set_center(layout, window, cx));
            }
            if let Some(panel) = zoom_tab.and_then(|id| this.panels.borrow().get(&id).cloned()) {
                let node = this
                    .dock
                    .read(cx)
                    .layout(DockPlacement::Center)
                    .and_then(|tree| tree.find_panel_node(PanelId::from(panel.entity_id())));
                if let Some(node) = node {
                    this.dock
                        .update(cx, |dock, cx| dock.set_zoomed_in(node, window, cx));
                }
            }
            let active = saved.groups.active(saved.groups.focused_group());
            this.restore_focus = active;
            let selected = saved
                .groups
                .active_tabs()
                .into_iter()
                .filter_map(|(_, id)| id)
                .collect();
            this.restore_queue =
                crate::session::restore_order(saved.tabs, active, &selected).into();
            this.start_restores(cx);
        }
        if let Some(path) = initial {
            this.open_file(path, window, cx);
        }
        if this.panels.borrow().is_empty() {
            this.new_terminal(this.workspace.linux_root.clone(), window, cx);
        }
        this
    }
    #[cfg(feature = "ui-tests")]
    pub fn focused_tab(&self) -> Option<TabId> {
        self.focused
    }
    #[cfg(feature = "ui-tests")]
    pub fn session_restore_complete(&self) -> bool {
        self.restore_queue.is_empty() && self.restoring.is_empty() && self.restore_focus.is_none()
    }

    #[cfg(feature = "ui-tests")]
    pub fn pane_is_zoomed(&self, cx: &App) -> bool {
        self.dock.read(cx).is_zoomed()
    }
    #[cfg(feature = "ui-tests")]
    pub fn disable_session_persistence_for_test(&mut self) {
        self.session_path = None;
    }
    #[cfg(feature = "ui-tests")]
    pub fn git_group_summary(&self) -> Vec<(PathBuf, usize)> {
        self.git_groups
            .iter()
            .map(|group| (group.root.clone(), group.changes.len()))
            .collect()
    }
    fn schedule_save(&mut self) {
        if self.closing {
            return;
        }
        self.save_at = Some(Instant::now() + Duration::from_secs(1));
    }
    fn snapshot(&self, cx: &App) -> Option<Session> {
        if !self.restore_queue.is_empty() || !self.restoring.is_empty() {
            return None;
        }
        let tabs = self
            .panels
            .borrow()
            .values()
            .map(|panel| panel.read(cx).saved())
            .collect();
        let mut saved = Session::new(
            self.workspace.distro.clone(),
            self.workspace.linux_root.clone(),
            tabs,
            self.legacy_groups.clone(),
        );
        saved.window_width = self.geometry.0;
        saved.window_height = self.geometry.1;
        saved.maximized = self.geometry.2;
        saved.sidebar_width = self.sidebar_width;
        saved.sidebar_view = i32::from(self.sidebar_git);
        let area = self.dock.read(cx);
        saved.maximized_group = area
            .zoomed_group()
            .and_then(|node| {
                area.layout(DockPlacement::Center)
                    .and_then(|tree| tree.find_node(node))
            })
            .and_then(|node| match node.kind() {
                dock::PaneRef::Tabs { panels, active_ix } => panels.get(active_ix).copied(),
                _ => None,
            })
            .and_then(|id| {
                self.panels
                    .borrow()
                    .iter()
                    .find(|(_, panel)| PanelId::from(panel.entity_id()) == id)
                    .map(|(id, _)| *id as usize)
            });
        saved.gpui_layout = serde_json::to_value(self.dock.read(cx).dump(cx)).ok();
        Some(saved)
    }
    fn create_panel(&mut self, tab: SavedTab, cx: &mut Context<Self>) -> Entity<WorkspacePanel> {
        let id = tab.id;
        self.next_id = self.next_id.max(id.saturating_add(1));
        if let SavedKind::Terminal { number } = tab.kind {
            self.next_terminal = self.next_terminal.max(number + 1);
        }
        let font_size = if matches!(tab.kind, SavedKind::Terminal { .. }) {
            self.fonts.terminal
        } else {
            self.fonts.editor
        } as f32;
        let terminal = matches!(tab.kind, SavedKind::Terminal { .. });
        let brightness = if terminal {
            self.fonts.terminal_brightness
        } else {
            self.fonts.editor_brightness
        };
        let panel = cx.new(|cx| WorkspacePanel::new(tab, font_size, cx));
        panel.update(cx, |panel, cx| panel.set_font(font_size, brightness, cx));
        let subscription = cx.subscribe(&panel, |this, _, event: &ContentEvent, cx| {
            match event {
                ContentEvent::Closed(id) => {
                    this.panels.borrow_mut().remove(id);
                    this.subscriptions.remove(id);
                    this.legacy_groups.remove(*id);
                }
                ContentEvent::RequestClose(id) => {
                    let id = *id;
                    let root = cx.entity();
                    cx.defer(move |cx| {
                        if let Some(window) = cx.windows().first().copied() {
                            let _ = window.update(cx, |_, window, cx| {
                                root.update(cx, |this, cx| this.close_id(id, window, cx))
                            });
                        }
                    });
                }
                ContentEvent::Focused(id) => {
                    this.focused = Some(*id);
                    this.legacy_groups.activate(*id);
                }
                ContentEvent::Message(message) => this.status = message.clone(),
                ContentEvent::Changed => {}
                ContentEvent::RefreshTree => this.refresh_tree(),
            }
            this.schedule_save();
            cx.notify();
        });
        self.subscriptions.insert(id, subscription);
        self.panels.borrow_mut().insert(id, panel.clone());
        panel
    }
    fn load_panel(&mut self, tab: SavedTab, cx: &mut Context<Self>) {
        let Some(panel) = self.panels.borrow().get(&tab.id).cloned() else {
            return;
        };
        let workspace = workspace_for(&self.workspace, &self.added, &tab.path).clone();
        let repository = self
            .repositories
            .iter()
            .filter(|root| tab.path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .cloned()
            .unwrap_or(workspace.linux_root.clone());
        let status = self.statuses.get(&tab.path).copied().unwrap_or_default();
        panel.update(cx, |panel, _| {
            panel.load(move || {
                let host = workspace
                    .host_path(&tab.path)
                    .map_err(|e| format!("{e:#}"))?;
                match tab.kind {
                    SavedKind::Terminal { .. } => {
                        crate::terminal::TerminalSession::spawn(&workspace.distro, &tab.path)
                            .map(Opened::Terminal)
                    }
                    SavedKind::Diff => {
                        git::load_diff(&workspace, &repository, &tab.path, status).map(Opened::Diff)
                    }
                    _ => {
                        if let Some(kind) = crate::preview::kind_for(&tab.path) {
                            crate::preview::open(tab.path, host, kind).map(|loaded| {
                                Opened::Preview(crate::gpui_panel::PreparedPreview::new(loaded))
                            })
                        } else {
                            Document::open(tab.path, host).map(Opened::File)
                        }
                    }
                }
                .map_err(|error| format!("{error:#}"))
            })
        });
    }
    fn start_restores(&mut self, cx: &mut Context<Self>) {
        while self.restoring.len() < 4 {
            let Some(tab) = self.restore_queue.pop_front() else {
                break;
            };
            self.restoring.insert(tab.id);
            self.load_panel(tab, cx);
        }
    }
    fn append(&mut self, tab: SavedTab, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.create_panel(tab.clone(), cx);
        self.legacy_groups.add(tab.id, 0);
        let target = self
            .focused
            .and_then(|id| {
                self.panels
                    .borrow()
                    .get(&id)
                    .map(|panel| PanelId::from(panel.entity_id()))
            })
            .and_then(|id| {
                self.dock
                    .read(cx)
                    .layout(DockPlacement::Center)
                    .and_then(|tree| tree.find_panel_node(id))
            });
        self.dock.update(cx, |dock, cx| {
            let panel_id = PanelId::from(panel.entity_id());
            dock.add_panel_view(
                dock::panel_handle(panel),
                DockPlacement::Center,
                None,
                window,
                cx,
            );
            if let Some(node) = target {
                dock.move_panel(
                    panel_id,
                    dock::InsertTarget::Tabs {
                        node,
                        ix: None,
                        activate: true,
                    },
                    window,
                    cx,
                );
            }
        });
        self.load_panel(tab, cx);
        self.schedule_save();
        cx.notify();
    }
    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self
            .panels
            .borrow()
            .values()
            .find(|panel| {
                panel.read(cx).path == path
                    && !matches!(
                        panel.read(cx).kind,
                        SavedKind::Terminal { .. } | SavedKind::Diff
                    )
            })
            .cloned();
        if let Some(panel) = existing {
            let id = PanelId::from(panel.entity_id());
            self.dock
                .update(cx, |dock, cx| dock.select_panel(id, window, cx));
            return;
        }
        let tab = SavedTab {
            id: self.next_id,
            path: path.clone(),
            kind: if crate::preview::kind_for(&path).is_some() {
                SavedKind::Preview
            } else {
                SavedKind::File {
                    unsaved_text: None,
                    revision: None,
                }
            },
        };
        self.append(tab, window, cx);
    }
    fn new_terminal(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let number = self.next_terminal;
        self.next_terminal += 1;
        self.append(
            SavedTab {
                id: self.next_id,
                path,
                kind: SavedKind::Terminal { number },
            },
            window,
            cx,
        );
    }
    fn diff(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.append(
            SavedTab {
                id: self.next_id,
                path,
                kind: SavedKind::Diff,
            },
            window,
            cx,
        );
    }
    fn restart_monitors(&mut self) {
        let workspaces = std::iter::once(self.workspace.clone())
            .chain(self.added.clone())
            .collect::<Vec<_>>();
        self.statuses
            .retain(|path, _| workspaces.iter().any(|w| path.starts_with(&w.linux_root)));
        self.repositories
            .retain(|path| workspaces.iter().any(|w| path.starts_with(&w.linux_root)));
        self.rebuild_git_view();
        self.monitor_loader.request(move || {
            workspaces
                .into_iter()
                .map(|workspace| {
                    let monitor = git::StatusMonitor::spawn(&workspace).map_err(|e| e.to_string());
                    (workspace.linux_root, monitor)
                })
                .collect()
        });
    }
    fn refresh_tree(&mut self) {
        let workspace = self.workspace.clone();
        let added = self.added.clone();
        let expanded = self.expanded.clone();
        let statuses = self.statuses.clone();
        self.tree_pending = true;
        self.tree_loader.request(move || {
            tree::build_tree_with_folders(&workspace, &added, &expanded, &statuses)
                .map(|nodes| TreeResult { nodes })
                .map_err(|e| format!("{e:#}"))
        });
    }
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let gap = now.duration_since(self.last_poll);
        self.last_poll = now;
        if gap >= Duration::from_secs(5) {
            crate::diagnostics_log::event(&format!(
                "event loop resumed after {} ms",
                gap.as_millis()
            ));
        }
        if now.duration_since(self.last_heartbeat) >= Duration::from_secs(60) {
            crate::diagnostics_log::event("event loop heartbeat");
            self.last_heartbeat = now;
        }
        let geometry = window_geometry(window);
        if geometry != self.geometry {
            self.geometry = geometry;
            self.schedule_save();
        }
        let finished: Vec<_> = self
            .restoring
            .iter()
            .filter(|id| {
                self.panels
                    .borrow()
                    .get(id)
                    .is_none_or(|panel| !matches!(panel.read(cx).content, Content::Loading))
            })
            .copied()
            .collect();
        if !finished.is_empty() {
            for id in finished {
                self.restoring.remove(&id);
            }
            self.start_restores(cx);
            if self.restoring.is_empty() && self.restore_queue.is_empty() {
                if let Some(id) = self.restore_focus.take() {
                    if let Some(panel) = self.panels.borrow().get(&id) {
                        panel.focus_handle(cx).focus(window, cx);
                        self.focused = Some(id);
                        self.legacy_groups.activate(id);
                    }
                }
            }
            self.schedule_save();
        }
        if let Some(result) = self.tree_loader.poll() {
            self.tree_pending = false;
            match result {
                Ok(tree) => self.tree = tree.nodes,
                Err(error) => self.status = error,
            }
            cx.notify();
        }
        if let Some(monitors) = self.monitor_loader.poll() {
            self.monitors.clear();
            for (path, result) in monitors {
                match result {
                    Ok(monitor) => self.monitors.push((path, monitor)),
                    Err(error) => self.status = error,
                }
            }
        }
        let mut git_changed = false;
        for (root, monitor) in &mut self.monitors {
            if let Some(result) = monitor.poll_latest() {
                match result {
                    Ok(snapshot) => {
                        self.statuses.retain(|path, _| !path.starts_with(&*root));
                        self.repositories.retain(|path| !path.starts_with(&*root));
                        self.statuses.extend(snapshot.statuses);
                        self.repositories.extend(snapshot.repositories);
                        git_changed = true;
                    }
                    Err(error) => {
                        self.status = error;
                        cx.notify();
                    }
                }
            }
        }
        if git_changed {
            self.rebuild_git_view();
            self.refresh_tree();
            cx.notify();
        }
        if let Some(result) = self.action_loader.poll() {
            match result {
                Ok(ActionResult::EntryCreated {
                    path,
                    parent,
                    directory,
                }) => {
                    self.expanded.insert(parent);
                    self.refresh_tree();
                    for (_, monitor) in &mut self.monitors {
                        let _ = monitor.force_refresh();
                    }
                    if directory {
                        self.status = format!("Created folder {}", path.display());
                    } else {
                        self.open_file(path, window, cx);
                    }
                }
                Ok(ActionResult::TreeChanged(rename)) => {
                    if let Some((from, to)) = rename {
                        for panel in self.panels.borrow().values() {
                            panel.update(cx, |panel, cx| {
                                panel.path = tree::rebased_path(&panel.path, &from, &to);
                                if let Content::File { document, .. } = &mut panel.content {
                                    document.linux_path = panel.path.clone();
                                    if let Ok(host) =
                                        workspace_for(&self.workspace, &self.added, &panel.path)
                                            .host_path(&panel.path)
                                    {
                                        document.host_path = host;
                                    }
                                }
                                panel.title = panel
                                    .path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into();
                                cx.notify();
                            });
                        }
                    }
                    self.refresh_tree();
                    for (_, monitor) in &mut self.monitors {
                        let _ = monitor.force_refresh();
                    }
                }
                Ok(ActionResult::Folder(folder)) => {
                    if !self.added.iter().any(|w| w.linux_root == folder.linux_root)
                        && folder.linux_root != self.workspace.linux_root
                    {
                        self.added.push(folder);
                        self.save_folders();
                        self.restart_monitors();
                        self.refresh_tree();
                    }
                }
                Ok(ActionResult::ProcessStopped(message)) => {
                    self.status = message;
                    let workspace = self.workspace.clone();
                    self.diagnostics_loader
                        .request(move || wsl_diagnostics::collect(&workspace));
                }
                Ok(ActionResult::Message(message)) => self.status = message,
                Err(error) => self.status = error,
            }
            cx.notify();
        }
        if let Some(report) = self.diagnostics_loader.poll() {
            self.report = Some(report);
            self.status = "Diagnostics ready".into();
            cx.notify();
        }
        if let Some(result) = self.docker_loader.poll() {
            self.status = match result {
                Ok(usage) => usage
                    .into_iter()
                    .map(|u| format!("{}: {} CPU, {}", u.name, u.cpu, u.memory))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(error) => error,
            };
            cx.notify();
        }
        self.agent_tick = self.agent_tick.wrapping_add(1);
        if self.agent_tick % 30 == 0 && window.is_window_active() {
            let probes = self
                .panels
                .borrow()
                .iter()
                .filter_map(|(id, panel)| {
                    if let Content::Terminal(terminal) = &panel.read(cx).content {
                        Some((*id, terminal.read(cx).session.agent_probe()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if !probes.is_empty() {
                self.agent_loader
                    .request(move || crate::terminal::AgentProbe::detect_many(&probes));
            }
        }
        if let Some(statuses) = self.agent_loader.poll() {
            for (id, status) in statuses {
                if let Some(panel) = self.panels.borrow().get(&id) {
                    panel.update(cx, |panel, cx| {
                        panel.set_agent(status, cx);
                    });
                }
            }
        }
        if self
            .save_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.save_at = None;
            if let (Some(snapshot), Some(path)) = (self.snapshot(cx), self.session_path.clone()) {
                let writer = self.session_writer.clone();
                let generation = writer.reserve();
                cx.background_executor()
                    .spawn(async move {
                        if let Err(error) = writer.save(generation, &snapshot, &path) {
                            crate::diagnostics_log::event(&format!("session save failed: {error}"));
                        }
                    })
                    .detach();
            }
        }
        for error in [
            self.tree_loader.take_error(),
            self.monitor_loader.take_error(),
            self.action_loader.take_error(),
            self.diagnostics_loader.take_error(),
            self.agent_loader.take_error(),
        ]
        .into_iter()
        .flatten()
        {
            self.status = error;
            cx.notify();
        }
    }
    fn save_folders(&self) {
        if let Some(path) =
            workspace_history::settings_path(&self.workspace.distro, &self.workspace.linux_root)
        {
            let distro = self.workspace.distro.clone();
            let root = self.workspace.linux_root.clone();
            let folders = self
                .added
                .iter()
                .map(|w| w.linux_root.clone())
                .collect::<Vec<_>>();
            std::thread::spawn(move || {
                let _ = workspace_history::save(&path, &distro, &root, &folders);
            });
        }
    }
    fn activate_tree(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node) = self.tree.get(index).cloned() else {
            return;
        };
        self.tree_selection = Some(node.linux_path.clone());
        if node.is_directory {
            if !self.expanded.remove(&node.linux_path) {
                self.expanded.insert(node.linux_path);
            }
            self.refresh_tree();
        } else {
            self.open_file(node.linux_path, window, cx);
        }
        cx.notify();
    }
    fn dialog(
        &mut self,
        title: &str,
        action: DialogAction,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input =
            value.map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value)));
        if let Some(input) = &input {
            input.focus_handle(cx).focus(window, cx);
        }
        let subscription = input.as_ref().map(|input| {
            cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) && this.dialog.is_some() {
                    this.confirm(window, cx);
                }
            })
        });
        if input.is_none() {
            self.focus.focus(window, cx);
        }
        self.dialog = Some(Dialog {
            title: title.into(),
            action,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let value = dialog
            .input
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        if let DialogAction::Close(id) = dialog.action {
            if let Some(panel) = self.panels.borrow().get(&id).cloned() {
                panel.update(cx, |panel, _| panel.dirty = false);
                self.dock
                    .update(cx, |dock, cx| dock.remove_panel(panel, window, cx));
            }
            cx.notify();
            return;
        }
        let primary = self.workspace.clone();
        let added = self.added.clone();
        let statuses = self.statuses.clone();
        let repositories = self.repositories.clone();
        self.action_loader.request(move || {
            match dialog.action {
                DialogAction::Create(parent, directory) => tree::create_entry(
                    workspace_for(&primary, &added, &parent),
                    &parent,
                    &value,
                    directory,
                )
                .map(|path| ActionResult::EntryCreated {
                    path,
                    parent,
                    directory,
                }),
                DialogAction::Rename(path) => {
                    tree::rename_entry(workspace_for(&primary, &added, &path), &path, &value)
                        .map(|to| ActionResult::TreeChanged(Some((path, to))))
                }
                DialogAction::Delete(path) => {
                    tree::delete_entry(workspace_for(&primary, &added, &path), &path)
                        .map(|_| ActionResult::TreeChanged(None))
                }
                DialogAction::Discard(path) => {
                    let workspace = workspace_for(&primary, &added, &path);
                    let root = repositories
                        .iter()
                        .filter(|root| path.starts_with(root))
                        .max_by_key(|root| root.components().count())
                        .unwrap_or(&workspace.linux_root);
                    git::discard_change(
                        workspace,
                        root,
                        &path,
                        statuses.get(&path).copied().unwrap_or_default(),
                    )
                    .map(|_| ActionResult::TreeChanged(None))
                }
                DialogAction::AddFolder => {
                    Workspace::prepare(primary.distro, value.into()).map(ActionResult::Folder)
                }
                DialogAction::Kill(target, force) => {
                    wsl_diagnostics::terminate_process(&primary, &target, force)
                        .map(ActionResult::ProcessStopped)
                        .map_err(anyhow::Error::from)
                }
                DialogAction::Close(_) => unreachable!(),
            }
            .map_err(|error| format!("{error:#}"))
        });
        cx.notify();
    }
    fn browse(&mut self, _cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        self.action_loader.request(move || {
            #[cfg(windows)]
            let result = crate::folder_picker::pick_folder(0, &workspace);
            #[cfg(not(windows))]
            let result: anyhow::Result<Option<PathBuf>> = Ok(None);
            match result {
                Ok(Some(path)) => Workspace::prepare(workspace.distro, path)
                    .map(ActionResult::Folder)
                    .map_err(|e| e.to_string()),
                Ok(None) => Ok(ActionResult::Message("Folder selection cancelled".into())),
                Err(error) => Err(error.to_string()),
            }
        });
    }
    fn request_diagnostics(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        self.diagnostics_loader
            .request(move || wsl_diagnostics::collect(&workspace));
        self.diagnostics_open = true;
        self.status = "Collecting diagnostics…".into();
        cx.notify();
    }
    fn save_active(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self
            .focused
            .and_then(|id| self.panels.borrow().get(&id).cloned())
        {
            panel.update(cx, |panel, cx| panel.save(false, cx));
        }
    }
    fn cycle(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focused.and_then(|id| {
            self.panels
                .borrow()
                .get(&id)
                .map(|panel| PanelId::from(panel.entity_id()))
        });
        let area = self.dock.read(cx);
        let tree = area.layout(DockPlacement::Center);
        let panel_ids = focused
            .and_then(|id| {
                tree.and_then(|tree| {
                    tree.find_panel_node(id)
                        .and_then(|node| tree.find_node(node))
                })
            })
            .and_then(|node| {
                if let dock::PaneRef::Tabs { panels, .. } = node.kind() {
                    Some(panels.to_vec())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let ids = panel_ids
            .into_iter()
            .filter_map(|id| {
                self.panels
                    .borrow()
                    .iter()
                    .find(|(_, panel)| PanelId::from(panel.entity_id()) == id)
                    .map(|(id, _)| *id)
            })
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        let index = ids
            .iter()
            .position(|id| Some(*id) == self.focused)
            .unwrap_or(0);
        let id = ids[(index as i32 + delta).rem_euclid(ids.len() as i32) as usize];
        let panel = self.panels.borrow()[&id].clone();
        self.dock.update(cx, |dock, cx| {
            dock.select_panel(PanelId::from(panel.entity_id()), window, cx)
        });
    }
    fn close_active(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.focused {
            self.close_id(id, window, cx);
        }
    }
    fn close_id(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.panels.borrow().get(&id).cloned();
        let Some(panel) = panel else {
            return;
        };
        if panel.read(cx).dirty {
            let id = panel.read(cx).id;
            self.dialog(
                "Discard unsaved changes and close?",
                DialogAction::Close(id),
                None,
                window,
                cx,
            );
        } else {
            self.dock
                .update(cx, |dock, cx| dock.remove_panel(panel, window, cx));
        }
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.closing {
            return false;
        }
        let Some(snapshot) = self.snapshot(cx) else {
            self.status = "Restoring the session; please close again after loading finishes".into();
            cx.notify();
            return false;
        };
        let Some(path) = self.session_path.clone() else {
            return true;
        };

        self.closing = true;
        self.save_at = None;
        let writer = self.session_writer.clone();
        let generation = writer.reserve();
        let task = cx
            .background_executor()
            .spawn(async move { writer.save(generation, &snapshot, &path) });
        self.close_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => window.remove_window(),
                Err(error) => {
                    this.closing = false;
                    this.status = format!("Cannot save session: {error}");
                    cx.notify();
                }
            });
        }));
        cx.notify();
        false
    }
    fn tree_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let node = self.tree[index].clone();
        let color = match node.git_status {
            GitStatus::Added | GitStatus::Untracked => 0x80d7a0,
            GitStatus::Modified => 0xdac377,
            GitStatus::Deleted => 0xe58b8b,
            _ => 0xc5cbd4,
        };
        let path = node.linux_path.clone();
        let is_directory = node.is_directory;
        let parent = if is_directory {
            path.clone()
        } else {
            path.parent()
                .unwrap_or(&self.workspace.linux_root)
                .to_path_buf()
        };
        let external_target = parent.clone();
        let menu_path = path.clone();
        let menu_parent = parent.clone();
        let weak = cx.weak_entity();
        div()
            .id(("tree-row", index))
            .h(px(self.fonts.tree as f32 * 1.65 + 3.))
            .flex()
            .items_center()
            .pl(px((node.depth * 14 + 8) as f32))
            .pr_2()
            .text_size(px(self.fonts.tree as f32))
            .text_color(crate::gpui_editor::font_color(
                rgb(color).into(),
                self.fonts.tree_brightness,
            ))
            .hover(|style| style.bg(rgb(0x293442)))
            .gap_1()
            .child(div().w(px(10.)).child(if is_directory {
                if node.is_expanded { "▾" } else { "▸" }
            } else {
                ""
            }))
            .child(
                Icon::new(if is_directory {
                    if node.is_expanded {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    }
                } else {
                    IconName::File
                })
                .size(px(14.)),
            )
            .child(node.name)
            .on_click(cx.listener(move |this, _, window, cx| this.activate_tree(index, window, cx)))
            .on_drag(DragPath(path), |path, _, _, cx| cx.new(|_| path.clone()))
            .on_drop(cx.listener(move |this, source: &DragPath, _, cx| {
                let target = parent.clone();
                let source = source.0.clone();
                let primary = this.workspace.clone();
                let added = this.added.clone();
                this.action_loader.request(move || {
                    tree::move_file(
                        workspace_for(&primary, &added, &source),
                        &source,
                        workspace_for(&primary, &added, &target),
                        &target,
                    )
                    .map(|to| ActionResult::TreeChanged(Some((source, to))))
                    .map_err(|e| format!("{e:#}"))
                });
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                let sources = paths.paths().to_vec();
                let target = external_target.clone();
                let primary = this.workspace.clone();
                let added = this.added.clone();
                this.action_loader.request(move || {
                    let workspace = workspace_for(&primary, &added, &target);
                    for source in sources {
                        tree::copy_external_file(workspace, &target, &source)
                            .map_err(|e| format!("{e:#}"))?;
                    }
                    Ok(ActionResult::TreeChanged(None))
                });
                cx.notify();
            }))
            .context_menu(move |menu, _, _| {
                use gpui_kit::component::menu::PopupMenuItem;
                let terminal = menu_parent.clone();
                let root = weak.clone();
                let new_file = menu_parent.clone();
                let file_root = weak.clone();
                let new_folder = menu_parent.clone();
                let folder_root = weak.clone();
                let rename = menu_path.clone();
                let rename_root = weak.clone();
                let delete = menu_path.clone();
                let delete_root = weak.clone();
                let copy = menu_path.clone();
                let remove = menu_path.clone();
                let remove_root = weak.clone();
                menu.item(PopupMenuItem::new("Open Terminal Here").on_click(
                    move |_, window, cx| {
                        let _ = root.update(cx, |this, cx| {
                            this.new_terminal(terminal.clone(), window, cx)
                        });
                    },
                ))
                .item(
                    PopupMenuItem::new("New File…").on_click(move |_, window, cx| {
                        let _ = file_root.update(cx, |this, cx| {
                            this.dialog(
                                "New file",
                                DialogAction::Create(new_file.clone(), false),
                                Some(String::new()),
                                window,
                                cx,
                            )
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("New Folder…").on_click(move |_, window, cx| {
                        let _ = folder_root.update(cx, |this, cx| {
                            this.dialog(
                                "New folder",
                                DialogAction::Create(new_folder.clone(), true),
                                Some(String::new()),
                                window,
                                cx,
                            )
                        });
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Copy Linux Path").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.display().to_string()))
                    }),
                )
                .item(
                    PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
                        let _ = rename_root.update(cx, |this, cx| {
                            this.dialog(
                                "Rename",
                                DialogAction::Rename(rename.clone()),
                                Some(
                                    rename
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into(),
                                ),
                                window,
                                cx,
                            )
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
                        let _ = delete_root.update(cx, |this, cx| {
                            this.dialog(
                                "Delete this file or folder?",
                                DialogAction::Delete(delete.clone()),
                                None,
                                window,
                                cx,
                            )
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("Remove Added Folder").on_click(move |_, _, cx| {
                        let _ = remove_root.update(cx, |this, cx| {
                            this.added.retain(|w| w.linux_root != remove);
                            this.save_folders();
                            this.restart_monitors();
                            this.refresh_tree();
                            cx.notify();
                        });
                    }),
                )
            })
            .into_any_element()
    }
    fn create_entry_dialog(
        &mut self,
        directory: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent = self
            .tree_selection
            .as_ref()
            .and_then(|path| self.tree.iter().find(|node| &node.linux_path == path))
            .map(|node| {
                if node.is_directory {
                    node.linux_path.clone()
                } else {
                    node.linux_path
                        .parent()
                        .unwrap_or(&self.workspace.linux_root)
                        .to_path_buf()
                }
            })
            .unwrap_or_else(|| self.workspace.linux_root.clone());
        self.expanded.insert(parent.clone());
        self.dialog(
            if directory { "New folder" } else { "New file" },
            DialogAction::Create(parent, directory),
            Some(String::new()),
            window,
            cx,
        );
    }
    fn rebuild_git_view(&mut self) {
        let roots = std::iter::once(self.workspace.linux_root.clone())
            .chain(self.added.iter().map(|w| w.linux_root.clone()))
            .collect::<Vec<_>>();
        self.git_groups = git_view::groups(&self.statuses, &self.repositories, &roots);
        self.git_rows = git_view::rows(&self.git_groups, &self.collapsed_git);
    }
    fn git_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::base::TestSupportExt;
        let height = px(32.);
        match self.git_rows[index] {
            git_view::Row::Group(group) => {
                let group = &self.git_groups[group];
                let root = group.root.clone();
                let collapsed = self.collapsed_git.contains(&root);
                div()
                    .id(("git-repository", index))
                    .test_support()
                    .h(height)
                    .w_full()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .bg(rgb(0x1d2733))
                    .hover(|style| style.bg(rgb(0x293442)))
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(format!("Git repository {}", group.name))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .size(px(12.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_ellipsis()
                                    .child(group.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(0x98a5b5))
                                    .text_ellipsis()
                                    .child(format!(
                                        "PROJECT · {}",
                                        group.root.display().to_string().replace('\\', "/")
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_md()
                            .bg(rgb(0x2b4b65))
                            .text_size(px(10.))
                            .child(group.changes.len().to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed_git.remove(&root) {
                            this.collapsed_git.insert(root.clone());
                        }
                        this.git_rows = git_view::rows(&this.git_groups, &this.collapsed_git);
                        cx.notify();
                    }))
                    .into_any_element()
            }
            git_view::Row::Change { group, change } => {
                let change = &self.git_groups[group].changes[change];
                let path = change.path.clone();
                let discard = path.clone();
                let weak = cx.weak_entity();
                let color = match change.status {
                    GitStatus::Added | GitStatus::Untracked => 0x80d7a0,
                    GitStatus::Deleted => 0xe58b8b,
                    _ => 0xdac377,
                };
                div()
                    .id(("git-change", index))
                    .test_support()
                    .h(height)
                    .w_full()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl(px(22.))
                    .pr_2()
                    .text_size(px(self.fonts.tree as f32))
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(format!("Git change {}", change.path.display()))
                    .child(
                        Icon::new(IconName::File)
                            .size(px(14.))
                            .text_color(rgb(0x9eacbc)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .child(change.name.clone()),
                    )
                    .child(
                        div()
                            .max_w(px(self.sidebar_width * 0.4))
                            .text_ellipsis()
                            .text_size(px(10.))
                            .text_color(rgb(0x98a5b5))
                            .child(change.directory.clone()),
                    )
                    .child(
                        div()
                            .w(px(12.))
                            .flex_shrink_0()
                            .text_color(rgb(color))
                            .child(git_view::status_label(change.status)),
                    )
                    .hover(|style| style.bg(rgb(0x293442)))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.diff(path.clone(), window, cx)),
                    )
                    .context_menu(move |menu, _, _| {
                        let weak = weak.clone();
                        let path = discard.clone();
                        menu.item(
                            gpui_kit::component::menu::PopupMenuItem::new("Discard changes…")
                                .on_click(move |_, window, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.dialog(
                                            "Discard changes to this file?",
                                            DialogAction::Discard(path.clone()),
                                            None,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        )
                    })
                    .into_any_element()
            }
        }
    }
    fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let narrow = self.sidebar_width < 210.;
        let list = if self.sidebar_git {
            let root = cx.entity();
            let count = self.git_rows.len();
            let total = self
                .git_groups
                .iter()
                .map(|group| group.changes.len())
                .sum::<usize>();
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(
                    div()
                        .h(px(28.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_2()
                        .text_size(px(11.))
                        .child("SOURCE CONTROL")
                        .child(
                            div()
                                .px_1()
                                .rounded_md()
                                .bg(rgb(0x2b4b65))
                                .child(total.to_string()),
                        ),
                )
                .when(count == 0, |view| {
                    view.child(
                        div()
                            .p_2()
                            .text_size(px(12.))
                            .text_color(rgb(0x98a5b5))
                            .child("No changes"),
                    )
                })
                .child(
                    uniform_list("git-changes", count, move |range, _, cx| {
                        root.update(cx, |this, cx| {
                            range
                                .map(|index| this.git_row(index, cx))
                                .collect::<Vec<_>>()
                        })
                    })
                    .flex_1()
                    .min_h_0()
                    .min_w_0(),
                )
                .into_any_element()
        } else {
            let root = cx.entity();
            let count = self.tree.len();
            uniform_list("file-tree", count, move |range, _, cx| {
                root.update(cx, |this, cx| {
                    range
                        .map(|index| this.tree_row(index, cx))
                        .collect::<Vec<_>>()
                })
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(0x181f29))
            .border_r_1()
            .border_color(rgb(0x354253))
            .child(
                div()
                    .id("app-brand")
                    .h(px(crate::gpui_chrome::HEADER_HEIGHT))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .bg(rgb(0x181f29))
                    .window_control_area(WindowControlArea::Drag)
                    .child(img("araseo-app-icon.png").size(px(20.)))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .child("ARASEO"),
                    ),
            )
            .child(
                div()
                    .h(px(30.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(if narrow { 0. } else { 4. }))
                    .px_1()
                    .child(
                        Button::new("files-view")
                            .when(narrow, |button| button.icon(IconName::File).w(px(24.)))
                            .when(!narrow, |button| button.label("FILES"))
                            .accessibility_label("Files")
                            .tooltip("Files")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_git = false;
                                this.schedule_save();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("new-file")
                            .text_color(rgb(0x79b8ed))
                            .icon(gpui_kit::assets::IconName::FilePlus)
                            .accessibility_label("New File")
                            .tooltip("New File")
                            .small()
                            .ghost()
                            .w(px(24.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_entry_dialog(false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("new-folder")
                            .text_color(rgb(0x79b8ed))
                            .icon(gpui_kit::assets::IconName::FolderPlus)
                            .accessibility_label("New Folder")
                            .tooltip("New Folder")
                            .small()
                            .ghost()
                            .w(px(24.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_entry_dialog(true, window, cx)
                            })),
                    )
                    .child(
                        Button::new("add-project")
                            .text_color(rgb(0x79b8ed))
                            .icon(gpui_kit::assets::IconName::PanelsTopLeft)
                            .accessibility_label("Add Project")
                            .tooltip("Add Project / WSL Folder")
                            .small()
                            .ghost()
                            .w(px(24.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dialog(
                                    "Add WSL folder",
                                    DialogAction::AddFolder,
                                    Some(String::new()),
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(div().flex_1().min_w_0())
                    .child(
                        Button::new("git-view")
                            .accessibility_label("Git")
                            .label(if self.sidebar_width < 240. {
                                "GIT".to_string()
                            } else {
                                format!("GIT {}", self.statuses.len())
                            })
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_git = true;
                                this.schedule_save();
                                cx.notify();
                            })),
                    ),
            )
            .child(list)
            .into_any_element()
    }
    fn apply_fonts(&mut self, cx: &mut Context<Self>) {
        for panel in self.panels.borrow().values() {
            let terminal = matches!(panel.read(cx).kind, SavedKind::Terminal { .. });
            let (size, brightness) = if terminal {
                (self.fonts.terminal, self.fonts.terminal_brightness)
            } else {
                (self.fonts.editor, self.fonts.editor_brightness)
            };
            panel.update(cx, |panel, cx| panel.set_font(size as f32, brightness, cx));
        }
        if let Some(path) = crate::appearance::settings_path() {
            let sizes = FontSizes {
                terminal: self.fonts.terminal,
                editor: self.fonts.editor,
                tree: self.fonts.tree,
                terminal_brightness: self.fonts.terminal_brightness,
                editor_brightness: self.fonts.editor_brightness,
                tree_brightness: self.fonts.tree_brightness,
            };
            cx.background_executor()
                .spawn(async move {
                    let _ = sizes.save(&path);
                })
                .detach();
        }
        cx.notify();
    }
    fn settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = vec![div().text_lg().child("Appearance").into_any_element()];
        for (index, (name, size, brightness)) in [
            (
                "Terminal",
                self.fonts.terminal,
                self.fonts.terminal_brightness,
            ),
            ("Editor", self.fonts.editor, self.fonts.editor_brightness),
            ("File tree", self.fonts.tree, self.fonts.tree_brightness),
        ]
        .into_iter()
        .enumerate()
        {
            rows.push(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(format!("{name}: {size}px · {brightness}%"))
                    .child(
                        Button::new(("font-minus", index))
                            .label("−")
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let value = match index {
                                    0 => &mut this.fonts.terminal,
                                    1 => &mut this.fonts.editor,
                                    _ => &mut this.fonts.tree,
                                };
                                *value = (*value - 1).max(10);
                                this.apply_fonts(cx);
                            })),
                    )
                    .child(
                        Button::new(("font-plus", index))
                            .label("+")
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let value = match index {
                                    0 => &mut this.fonts.terminal,
                                    1 => &mut this.fonts.editor,
                                    _ => &mut this.fonts.tree,
                                };
                                *value = (*value + 1).min(if index == 2 { 24 } else { 28 });
                                this.apply_fonts(cx);
                            })),
                    )
                    .child(
                        Button::new(("brightness-minus", index))
                            .label("Dim")
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let value = match index {
                                    0 => &mut this.fonts.terminal_brightness,
                                    1 => &mut this.fonts.editor_brightness,
                                    _ => &mut this.fonts.tree_brightness,
                                };
                                *value = (*value - 10).max(50);
                                this.apply_fonts(cx);
                            })),
                    )
                    .child(
                        Button::new(("brightness-plus", index))
                            .label("Bright")
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let value = match index {
                                    0 => &mut this.fonts.terminal_brightness,
                                    1 => &mut this.fonts.editor_brightness,
                                    _ => &mut this.fonts.tree_brightness,
                                };
                                *value = (*value + 10).min(150);
                                this.apply_fonts(cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("reset-fonts")
                        .label("Reset defaults")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.fonts = FontSizes::default();
                            this.apply_fonts(cx);
                        })),
                )
                .child(
                    Button::new("close-settings")
                        .label("Close")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings_open = false;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        overlay(div().flex().flex_col().gap_4().children(rows))
    }
    fn diagnostics(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = vec![div().text_lg().child("WSL Diagnostics").into_any_element()];
        if let Some(report) = &self.report {
            for (index, check) in report.checks.iter().enumerate() {
                let mut row = div().flex().gap_2().child(
                    div()
                        .flex_1()
                        .child(format!("{}: {}", check.title, check.detail)),
                );
                if let Some(target) = &check.process {
                    let graceful = target.clone();
                    let force = target.clone();
                    row = row
                        .child(
                            Button::new(("terminate", index))
                                .label("Stop")
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.dialog(
                                        "Stop this process?",
                                        DialogAction::Kill(graceful.clone(), false),
                                        None,
                                        window,
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new(("force", index))
                                .label("Kill")
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.dialog(
                                        "Force kill this process?",
                                        DialogAction::Kill(force.clone(), true),
                                        None,
                                        window,
                                        cx,
                                    )
                                })),
                        );
                }
                rows.push(row.into_any_element());
            }
        } else {
            rows.push(div().child("Collecting…").into_any_element());
        }
        rows.push(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("refresh-diagnostics")
                        .label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.request_diagnostics(cx))),
                )
                .child(
                    Button::new("copy-diagnostics")
                        .label("Copy")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(report) = &this.report {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    report.copy_text(),
                                ));
                            }
                        })),
                )
                .child(
                    Button::new("docker-usage")
                        .label("Docker usage")
                        .on_click(cx.listener(|this, _, _, _| {
                            let workspace = this.workspace.clone();
                            this.docker_loader.request(move || {
                                wsl_diagnostics::collect_docker_usage(&workspace)
                                    .map_err(|e| e.to_string())
                            });
                        })),
                )
                .child(
                    Button::new("close-diagnostics")
                        .label("Close")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.diagnostics_open = false;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        overlay(
            div()
                .id("diagnostics-scroll")
                .flex()
                .flex_col()
                .gap_3()
                .max_h(px(650.))
                .overflow_y_scroll()
                .children(rows),
        )
    }
}
fn window_geometry(window: &Window) -> (u32, u32, bool) {
    let state = window.window_bounds();
    let bounds = state.get_bounds();
    (
        f32::from(bounds.size.width) as u32,
        f32::from(bounds.size.height) as u32,
        matches!(
            state,
            WindowBounds::Maximized(_) | WindowBounds::Fullscreen(_)
        ),
    )
}
fn overlay(content: impl IntoElement) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .bg(rgba(0x00000090))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(760.))
                .max_w_full()
                .p_5()
                .bg(rgb(0x202936))
                .border_1()
                .border_color(rgb(0x516478))
                .rounded_lg()
                .child(content),
        )
        .into_any_element()
}
fn workspace_for<'a>(primary: &'a Workspace, added: &'a [Workspace], path: &Path) -> &'a Workspace {
    workspace::for_path(std::iter::once(primary).chain(added), path).unwrap_or(primary)
}
fn valid_layout(state: &DockAreaState, ids: &HashSet<TabId>) -> bool {
    fn visit(
        state: &dock::PanelState,
        ids: &HashSet<TabId>,
        seen: &mut HashSet<TabId>,
        depth: usize,
    ) -> bool {
        if depth > 64 {
            return false;
        }
        match &state.info {
            PanelInfo::Stack { sizes, axis }
                if *axis > 1
                    || sizes
                        .iter()
                        .any(|size| !f32::from(*size).is_finite() || *size < px(0.)) =>
            {
                false
            }
            PanelInfo::Panel(value) => {
                state.panel_name == "AraseoTab"
                    && value
                        .get("id")
                        .and_then(|id| id.as_u64())
                        .and_then(|id| u32::try_from(id).ok())
                        .is_some_and(|id| ids.contains(&id) && seen.insert(id))
            }
            _ => state
                .children
                .iter()
                .all(|child| visit(child, ids, seen, depth + 1)),
        }
    }
    if state.version != Some(1)
        || [&state.left_dock, &state.right_dock, &state.bottom_dock]
            .into_iter()
            .flatten()
            .any(|dock| !f32::from(dock.size()).is_finite() || dock.size() < px(0.))
    {
        return false;
    }
    let mut seen = HashSet::new();
    let valid = visit(&state.center, ids, &mut seen, 0)
        && [&state.left_dock, &state.right_dock, &state.bottom_dock]
            .into_iter()
            .flatten()
            .all(|dock| visit(dock.panel(), ids, &mut seen, 0));
    valid && seen == *ids
}
fn legacy_layout(
    spec: &LayoutSpec,
    groups: &TabGroups,
    panels: &HashMap<TabId, Entity<WorkspacePanel>>,
    cx: &App,
) -> DockLayout {
    match spec {
        LayoutSpec::Group(group) => {
            let mut layout = DockLayout::tabs();
            let ids = groups.group_ids(*group);
            let active = ids
                .iter()
                .position(|id| Some(*id) == groups.active(*group))
                .unwrap_or(0);
            for id in ids {
                if let Some(panel) = panels.get(&id) {
                    layout = layout.panel_view(dock::panel_handle(panel.clone()), cx);
                }
            }
            layout.active_index(active)
        }
        LayoutSpec::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let layout = if *axis == crate::tabs::Axis::Horizontal {
                DockLayout::h_split()
            } else {
                DockLayout::v_split()
            };
            layout
                .child(
                    legacy_layout(first, groups, panels, cx),
                    Some(px(f32::from(*ratio))),
                )
                .child(
                    legacy_layout(second, groups, panels, cx),
                    Some(px(1000. - f32::from(*ratio))),
                )
        }
    }
}
impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.caption_node
            .set(crate::gpui_chrome::caption_node(self.dock.read(cx)));
        let sidebar = self.sidebar(cx);
        let settings = self.settings_open.then(|| self.settings(cx));
        let diagnostics =
            (self.diagnostics_open && !self.settings_open).then(|| self.diagnostics(cx));
        let dialog = self.dialog.as_ref().map(|dialog| {
            overlay(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(dialog.title.clone())
                    .children(dialog.input.as_ref().map(|input| Input::new(input)))
                    .when(matches!(dialog.action, DialogAction::AddFolder), |view| {
                        view.child(Button::new("browse-folder").label("Browse…").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.dialog = None;
                                this.browse(cx);
                                cx.notify();
                            }),
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Button::new("confirm-dialog").label("Confirm").on_click(
                                cx.listener(|this, _, window, cx| this.confirm(window, cx)),
                            ))
                            .child(Button::new("cancel-dialog").label("Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                }),
                            )),
                    ),
            )
        });
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x10151d))
            .text_color(rgb(0xcbd3de))
            .text_size(px(13.))
            .font_family("Segoe UI")
            .track_focus(&self.focus)
            .key_context("Araseo")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && (this.dialog.is_some() || this.settings_open || this.diagnostics_open)
                {
                    this.dialog = None;
                    this.settings_open = false;
                    this.diagnostics_open = false;
                    if let Some(panel) = this
                        .focused
                        .and_then(|id| this.panels.borrow().get(&id).cloned())
                    {
                        panel.focus_handle(cx).focus(window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                } else if event.keystroke.key == "enter"
                    && this
                        .dialog
                        .as_ref()
                        .is_some_and(|dialog| dialog.input.is_none())
                {
                    this.confirm(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.resizing_sidebar {
                    this.sidebar_width = f32::from(event.position.x).clamp(
                        150.,
                        (f32::from(window.bounds().size.width) * 0.6).max(150.),
                    );
                    this.schedule_save();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resizing_sidebar = false),
            )
            .on_action(cx.listener(Self::save_active))
            .on_action(cx.listener(Self::close_active))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| {
                this.new_terminal(this.workspace.linux_root.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle(-1, window, cx)))
            .on_action(cx.listener(|this, _: &Diagnostics, _, cx| this.request_diagnostics(cx)))
            .on_action(cx.listener(|this, _: &Settings, _, cx| {
                this.settings_open = !this.settings_open;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(
                        div()
                            .id("sidebar-resizer")
                            .w(px(4.))
                            .h_full()
                            .cursor(CursorStyle::ResizeLeftRight)
                            .hover(|style| style.bg(rgb(0x6090d0)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.resizing_sidebar = true;
                                    cx.stop_propagation();
                                }),
                            ),
                    )
                    .child(div().flex_1().min_w_0().child(self.dock.clone())),
            )
            .child(
                div()
                    .h(px(27.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .bg(rgb(0x1c2633))
                    .child(
                        Button::new("wsl")
                            .label("WSL")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.request_diagnostics(cx))),
                    )
                    .child(
                        Button::new("appearance")
                            .label("Aa")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_open = true;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().overflow_hidden().child(self.status.clone())),
            )
            .children(settings)
            .children(diagnostics)
            .children(dialog)
    }
}

pub fn run(workspace: Workspace, initial: Option<PathBuf>) -> anyhow::Result<()> {
    let session_path = crate::session::settings_path(&workspace.distro, &workspace.linux_root);
    let saved = session_path
        .as_deref()
        .and_then(|path| Session::load(path, &workspace.distro, &workspace.linux_root));
    let width = saved
        .as_ref()
        .map(|s| s.window_width.clamp(640, 7680))
        .unwrap_or(1280) as f32;
    let height = saved
        .as_ref()
        .map(|s| s.window_height.clamp(480, 4320))
        .unwrap_or(760) as f32;
    let failure = Rc::new(RefCell::new(None));
    let startup_failure = failure.clone();
    gpui_kit::application()
        .with_assets(crate::gpui_chrome::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            crate::gpui_terminal::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            cx.bind_keys([
                KeyBinding::new("ctrl-s", Save, Some("Araseo")),
                KeyBinding::new("ctrl-shift-t", NewTerminal, Some("Araseo")),
                KeyBinding::new("ctrl-tab", NextTab, Some("Araseo")),
                KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Araseo")),
                KeyBinding::new("ctrl-w", CloseTab, Some("Araseo")),
            ]);
            let available = cx
                .primary_display()
                .map(|display| display.bounds().size)
                .unwrap_or(size(px(1280.), px(800.)));
            let width = width.min((f32::from(available.width) - 32.).max(640.));
            let height = height.min((f32::from(available.height) - 80.).max(480.));
            let options = WindowOptions {
                window_bounds: Some(if saved.as_ref().is_some_and(|s| s.maximized) {
                    WindowBounds::Maximized(Bounds::centered(None, size(px(width), px(height)), cx))
                } else {
                    WindowBounds::Windowed(Bounds::centered(None, size(px(width), px(height)), cx))
                }),
                titlebar: Some(TitlebarOptions {
                    title: Some("Araseo".into()),
                    ..gpui_kit::component::TitleBar::title_bar_options()
                }),
                window_min_size: Some(size(px(640.), px(480.))),
                ..gpui_kit::component::TitleBar::window_options()
            };
            match gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Workbench::new(workspace, saved, initial, window, cx))
            }) {
                Ok(_) => {
                    crate::diagnostics_log::event("GPUI DirectX event loop started");
                    cx.background_executor()
                        .spawn(async {
                            crate::diagnostics_log::capture_modules();
                        })
                        .detach();
                    cx.activate(true);
                }
                Err(error) => {
                    crate::diagnostics_log::event(&format!(
                        "GPUI window initialization failed: {error:#}"
                    ));
                    *startup_failure.borrow_mut() = Some(error);
                    cx.quit();
                }
            }
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        });
    if let Some(error) = failure.borrow_mut().take() {
        return Err(error);
    }
    Ok(())
}
