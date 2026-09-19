#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod appearance;
mod background;
mod document;
mod editor_view;
mod emoji;
mod git;
mod highlight;
mod tabs;
mod terminal;
mod tree;
mod workspace;

use anyhow::{Context, Result, bail};
use background::Background;
use document::{Document, ExternalRefresh};
use slint::winit_030::WinitWindowAccessor;
use slint::{Model, ModelRc, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tabs::{Axis, Dock, Rect, TabGroups, TabId};
use terminal::TerminalSession;
use tree::{FlatNode, GitStatus};
use workspace::Workspace;

slint::include_modules!();

struct AppState {
    workspace: Workspace,
    expanded: HashSet<PathBuf>,
    expanded_git_repositories: HashSet<PathBuf>,
    statuses: HashMap<PathBuf, GitStatus>,
    repositories: HashSet<PathBuf>,
    tree: Vec<FlatNode>,
    tabs: Vec<WorkspaceTab>,
    tab_groups: TabGroups,
    next_tab_id: TabId,
    next_terminal_number: u32,
    git_monitor: Option<git::StatusMonitor>,
    status: String,
    save_conflict: Option<TabId>,
    syncing_editor: Cell<bool>,
    emoji_icons: emoji::EmojiIcons,
    tree_loader: Background<Result<Vec<FlatNode>>>,
    file_loader: Background<Result<(Document, usize)>>,
    diff_loader: Background<Result<(git::FileDiff, usize)>>,
    git_action_loader: Background<Result<PathBuf>>,
    tree_action_loader: Background<Result<TreeActionResult>>,
    tree_action_pending: bool,
    editor_views: RefCell<[editor_view::EditorView; 2]>,
    extra_editor_views: RefCell<HashMap<usize, editor_view::EditorView>>,
    extra_terminal_sizes: HashMap<usize, (u16, u16)>,
    maximized_group: Option<usize>,
}

enum TreeActionResult {
    Created {
        path: PathBuf,
        parent: PathBuf,
        is_directory: bool,
    },
    Deleted(PathBuf),
    Renamed {
        from: PathBuf,
        to: PathBuf,
        from_host: PathBuf,
        to_host: PathBuf,
    },
    Copied {
        path: PathBuf,
        parent: PathBuf,
    },
}

enum TabContent {
    File(Document),
    Diff(git::FileDiff),
    Terminal {
        session: TerminalSession,
        start_path: PathBuf,
        number: u32,
    },
}

struct WorkspaceTab {
    id: TabId,
    content: TabContent,
}

fn main() -> Result<()> {
    let (distro, root, initial_file) = parse_args()?;
    // Keep the production renderer identical to the behavioral UI harness.
    // In particular, this avoids renderer-specific handling of a transparent
    // editable text layer placed above syntax-highlighted text.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
        .context("failed to initialize the Windows software renderer")?;
    let workspace = Workspace::prepare(distro, root)?;
    let statuses = HashMap::new();
    let tree = Vec::new();
    let mut startup_loader = Background::default();
    let startup_workspace = workspace.clone();
    startup_loader.request(move || -> Result<_> {
        let workspace = Workspace::new(startup_workspace.distro, startup_workspace.linux_root)?;
        Ok((
            git::StatusMonitor::spawn(&workspace),
            TerminalSession::spawn(&workspace.distro, &workspace.linux_root),
        ))
    });
    let state = Rc::new(RefCell::new(AppState {
        workspace,
        expanded: HashSet::new(),
        expanded_git_repositories: HashSet::new(),
        statuses,
        repositories: HashSet::new(),
        tree,
        tabs: Vec::new(),
        tab_groups: TabGroups::default(),
        next_tab_id: 0,
        next_terminal_number: 1,
        git_monitor: None,
        status: "Starting workspace...".into(),
        save_conflict: None,
        syncing_editor: Cell::new(false),
        emoji_icons: emoji::EmojiIcons::default(),
        tree_loader: Background::default(),
        file_loader: Background::default(),
        diff_loader: Background::default(),
        git_action_loader: Background::default(),
        tree_action_loader: Background::default(),
        tree_action_pending: false,
        editor_views: RefCell::new(Default::default()),
        extra_editor_views: RefCell::new(HashMap::new()),
        extra_terminal_sizes: HashMap::new(),
        maximized_group: None,
    }));

    refresh_tree(&mut state.borrow_mut());
    let mut icon_loader = Background::default();
    icon_loader.request(emoji::EmojiIcons::load_pixels);

    let ui = AppWindow::new()?;
    ui.set_dynamic_panes(true);
    ui.set_extra_panes(ModelRc::new(VecModel::from(Vec::<PaneEntry>::new())));
    install_windows_file_drop(&ui, state.clone());
    if let Some(path) = appearance::settings_path() {
        let sizes = appearance::FontSizes::load(&path);
        ui.set_terminal_font_size(sizes.terminal);
        ui.set_editor_font_size(sizes.editor);
        ui.set_tree_font_size(sizes.tree);
        ui.set_terminal_font_brightness(sizes.terminal_brightness);
        ui.set_editor_font_brightness(sizes.editor_brightness);
        ui.set_tree_font_brightness(sizes.tree_brightness);
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_font_settings_changed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let sizes = appearance::FontSizes {
                terminal: ui.get_terminal_font_size(),
                editor: ui.get_editor_font_size(),
                tree: ui.get_tree_font_size(),
                terminal_brightness: ui.get_terminal_font_brightness(),
                editor_brightness: ui.get_editor_font_brightness(),
                tree_brightness: ui.get_tree_font_brightness(),
            };
            if let Err(error) = sizes.save(&path) {
                ui.set_status_text(format!("Could not save font settings: {error}").into());
            }
            let state = state.borrow();
            for group in state.tab_groups.groups() {
                sync_group(&ui, &state, group);
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_terminal_output_ready(move |tab_id| {
            let Ok(tab_id) = TabId::try_from(tab_id) else {
                return;
            };
            let Some(ui) = weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            let visible_group = state
                .tab_groups
                .group_of(tab_id)
                .filter(|group| state.tab_groups.active(*group) == Some(tab_id));
            let changed = terminal_mut(&mut state, tab_id).is_some_and(TerminalSession::poll);
            if changed && let Some(group) = visible_group {
                sync_group(&ui, &state, group);
            }
        });
    }
    if let Some(path) = initial_file {
        let _ = open_document(&mut state.borrow_mut(), path);
    }
    sync_ui(&ui, &state.borrow());

    {
        let weak = ui.as_weak();
        ui.on_window_drag_requested(move || {
            if let Some(ui) = weak.upgrade() {
                ui.window().with_winit_window(|window| {
                    let _ = window.drag_window();
                });
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tree_activated(move |index| {
            let mut state = state.borrow_mut();
            if let Some(node) = state.tree.get(index as usize).cloned() {
                if node.is_directory {
                    if !state.expanded.remove(&node.linux_path) {
                        state.expanded.insert(node.linux_path);
                    }
                    refresh_tree(&mut state);
                } else if let Err(error) = open_document(&mut state, node.linux_path) {
                    state.status = error.to_string();
                }
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tree_create_requested(move |target, name, is_directory| {
            let mut state = state.borrow_mut();
            if state.tree_action_pending {
                state.status = "A file tree action is already running".into();
            } else {
                let parent = if target.is_empty() {
                    Some(state.workspace.linux_root.clone())
                } else {
                    tree_node_for_ui_path(&state, target.as_str()).and_then(|node| {
                        if node.is_directory {
                            Some(node.linux_path)
                        } else {
                            node.linux_path.parent().map(PathBuf::from)
                        }
                    })
                };
                if let Some(parent) = parent {
                    let workspace = state.workspace.clone();
                    let result_parent = parent.clone();
                    let result_name = name.to_string();
                    state.tree_action_pending = true;
                    state.status = format!(
                        "Creating {} {}...",
                        if is_directory { "folder" } else { "file" },
                        result_name
                    );
                    state.tree_action_loader.request(move || {
                        let path =
                            tree::create_entry(&workspace, &parent, &result_name, is_directory)?;
                        Ok(TreeActionResult::Created {
                            path,
                            parent: result_parent,
                            is_directory,
                        })
                    });
                } else {
                    state.status = "The selected file tree item is no longer available".into();
                }
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tree_rename_requested(move |path, new_name| {
            let mut state = state.borrow_mut();
            if state.tree_action_pending {
                state.status = "A file tree action is already running".into();
            } else if let Some(node) = tree_node_for_ui_path(&state, path.as_str()) {
                let workspace = state.workspace.clone();
                let from = node.linux_path;
                let name = new_name.to_string();
                state.tree_action_pending = true;
                state.status = format!("Renaming {}...", from.display());
                state.tree_action_loader.request(move || {
                    let from_host = workspace.host_path(&from)?;
                    let to = tree::rename_entry(&workspace, &from, &name)?;
                    let to_host = from_host.with_file_name(&name);
                    Ok(TreeActionResult::Renamed {
                        from,
                        to,
                        from_host,
                        to_host,
                    })
                });
            } else {
                state.status = "The selected file tree item is no longer available".into();
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tree_delete_requested(move |path| {
            let mut state = state.borrow_mut();
            if state.tree_action_pending {
                state.status = "A file tree action is already running".into();
            } else if let Some(node) = tree_node_for_ui_path(&state, path.as_str()) {
                if has_dirty_document_at_or_below(&state, &node.linux_path) {
                    state.status = "Save or close modified files before deleting".into();
                } else {
                    let workspace = state.workspace.clone();
                    let path = node.linux_path;
                    let result_path = path.clone();
                    state.tree_action_pending = true;
                    state.status = format!("Deleting {}...", path.display());
                    state.tree_action_loader.request(move || {
                        tree::delete_entry(&workspace, &path)?;
                        Ok(TreeActionResult::Deleted(result_path))
                    });
                }
            } else {
                state.status = "The selected file tree item is no longer available".into();
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tree_path_copied(move |path| {
            let mut state = state.borrow_mut();
            state.status = format!("Copied path: {path}");
            if let Some(ui) = weak.upgrade() {
                ui.set_status_text(state.status.clone().into());
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_git_repository_toggled(move |path| {
            let mut state = state.borrow_mut();
            let path = PathBuf::from(path.as_str());
            if !state.expanded_git_repositories.remove(&path) {
                state.expanded_git_repositories.insert(path);
            }
            if let Some(ui) = weak.upgrade() {
                sync_git_changes(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_git_change_activated(move |path| {
            let mut state = state.borrow_mut();
            if let Err(error) = open_git_diff(&mut state, PathBuf::from(path.as_str())) {
                state.status = error.to_string();
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_git_discard_requested(move |path| {
            let mut state = state.borrow_mut();
            let path = PathBuf::from(path.as_str());
            let Some(status) = state.statuses.get(&path).copied() else {
                state.status = "Git change is no longer available".into();
                if let Some(ui) = weak.upgrade() {
                    sync_ui(&ui, &state);
                }
                return;
            };
            let Some(repository) = repository_for(&state.repositories, &path) else {
                state.status = format!("Cannot find repository for {}", path.display());
                if let Some(ui) = weak.upgrade() {
                    sync_ui(&ui, &state);
                }
                return;
            };
            let workspace = state.workspace.clone();
            let result_path = path.clone();
            state.status = format!("Discarding {}...", path.display());
            state.git_action_loader.request(move || {
                git::discard_change(&workspace, &repository, &path, status)?;
                Ok(result_path)
            });
            if let Some(ui) = weak.upgrade() {
                ui.set_status_text(state.status.clone().into());
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_undo_requested(move |tab_id| {
            apply_history_change(&weak, &state, tab_id, true);
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_redo_requested(move |tab_id| {
            apply_history_change(&weak, &state, tab_id, false);
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tab_cycle(move |delta| {
            let mut state = state.borrow_mut();
            cancel_file_open(&mut state);
            state.tab_groups.cycle(delta);
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tab_activated(move |tab_id| {
            let mut state = state.borrow_mut();
            cancel_file_open(&mut state);
            if let Ok(tab_id) = TabId::try_from(tab_id) {
                state.tab_groups.activate(tab_id);
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tab_close(move |tab_id| {
            let mut state = state.borrow_mut();
            if let Ok(tab_id) = TabId::try_from(tab_id) {
                close_tab(&mut state, tab_id);
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_editor_edited(move |text, tab_id| {
            if state.borrow().syncing_editor.get() {
                return;
            }
            let mut state = state.borrow_mut();
            let Ok(tab_id) = TabId::try_from(tab_id) else {
                return;
            };
            let is_active = state
                .tab_groups
                .group_of(tab_id)
                .is_some_and(|group| state.tab_groups.active(group) == Some(tab_id));
            let mut dirty_changed = false;
            if is_active && let Some(document) = document_mut(&mut state, tab_id) {
                let was_dirty = document.dirty;
                document.set_text(text.to_string());
                dirty_changed = was_dirty != document.dirty;
            }
            if let Some(ui) = weak.upgrade() {
                if dirty_changed {
                    sync_tabs(&ui, &state);
                }
                if is_active
                    && let Some(group) = state.tab_groups.group_of(tab_id)
                    && let Some(document) = document_ref(&state, tab_id)
                {
                    // TextInput already owns the new text. Do not feed the
                    // entire buffer back through Slint on every keystroke.
                    sync_editor_view(&ui, &state, document, group, text);
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_save_requested(move |tab_id| {
            let mut state = state.borrow_mut();
            if let Ok(tab_id) = TabId::try_from(tab_id) {
                save_tab(&mut state, tab_id, false);
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_force_save_requested(move || {
            let mut state = state.borrow_mut();
            if let Some(tab_id) = state.save_conflict.or_else(|| focused_tab_id(&state)) {
                save_tab(&mut state, tab_id, true);
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_reload_requested(move || {
            let mut state = state.borrow_mut();
            if let Some(tab_id) = state.save_conflict.or_else(|| focused_tab_id(&state)) {
                reload_tab(&mut state, tab_id);
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_terminal_key(move |tab_id, text, control, alt, shift| {
            let Ok(tab_id) = TabId::try_from(tab_id) else {
                return;
            };
            let mut state = state.borrow_mut();
            let visible_group = state
                .tab_groups
                .group_of(tab_id)
                .filter(|group| state.tab_groups.active(*group) == Some(tab_id));
            let returned_to_bottom = terminal_mut(&mut state, tab_id).is_some_and(|terminal| {
                terminal.write(&terminal::encode_key(&text, control, alt, shift))
            });
            if returned_to_bottom && let (Some(ui), Some(group)) = (weak.upgrade(), visible_group) {
                sync_group(&ui, &state, group);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_terminal_text(move |tab_id, text| {
            let Ok(tab_id) = TabId::try_from(tab_id) else {
                return;
            };
            let mut state = state.borrow_mut();
            let visible_group = state
                .tab_groups
                .group_of(tab_id)
                .filter(|group| state.tab_groups.active(*group) == Some(tab_id));
            let returned_to_bottom = terminal_mut(&mut state, tab_id)
                .is_some_and(|terminal| terminal.write(text.as_bytes()));
            if returned_to_bottom && let (Some(ui), Some(group)) = (weak.upgrade(), visible_group) {
                sync_group(&ui, &state, group);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_terminal_scrollback(move |tab_id, rows| {
            let Ok(tab_id) = TabId::try_from(tab_id) else {
                return;
            };
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            let visible_group = state
                .tab_groups
                .group_of(tab_id)
                .filter(|group| state.tab_groups.active(*group) == Some(tab_id));
            let changed = terminal_mut(&mut state, tab_id)
                .is_some_and(|terminal| terminal.scroll_scrollback(rows));
            if changed && let Some(group) = visible_group {
                sync_group(&ui, &state, group);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_find_requested(move |tab_id, query| {
            let mut state = state.borrow_mut();
            let result = TabId::try_from(tab_id)
                .ok()
                .and_then(|tab_id| document_ref(&state, tab_id))
                .and_then(|document| document.text.find(query.as_str()));
            state.status = match result {
                Some(offset) if !query.is_empty() => format!("Found at byte {}", offset + 1),
                _ if query.is_empty() => "Find".into(),
                _ => "No match".into(),
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_status_text(state.status.clone().into());
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_new_terminal_requested(move |group| {
            let mut state = state.borrow_mut();
            let group = usize::try_from(group).unwrap_or(0);
            match open_terminal(&mut state, group) {
                Ok(tab_id) => {
                    if let Some(ui) = weak.upgrade()
                        && let Some(session) = terminal_ref(&state, tab_id)
                    {
                        connect_terminal_output(&ui, tab_id, session);
                    }
                }
                Err(error) => state.status = format!("Terminal unavailable: {error}"),
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
                ui.invoke_focus_terminal();
            }
        });
    }
    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_dock_target_requested(move |tab_id, x, y| {
            let Some(ui) = weak.upgrade() else {
                return invalid_dock_target();
            };
            let state = state.borrow();
            let Some(source) = TabId::try_from(tab_id)
                .ok()
                .and_then(|id| state.tab_groups.group_of(id))
            else {
                return invalid_dock_target();
            };
            let (local_x, local_y, width, height) = workspace_pointer(&ui, x, y);
            let Some((group, rect)) = visible_panes(&state)
                .into_iter()
                .find(|(_, rect)| rect.contains(local_x, local_y))
            else {
                return invalid_dock_target();
            };
            let within_x = (local_x - rect.x) / rect.width;
            let within_y = (local_y - rect.y) / rect.height;
            let zone = if within_x < 0.25 {
                0
            } else if within_x > 0.75 {
                1
            } else if within_y < 0.25 {
                2
            } else if within_y > 0.75 {
                3
            } else {
                4
            };
            if source == group && (zone == 4 || state.tab_groups.group_ids(source).len() == 1) {
                return invalid_dock_target();
            }
            if zone <= 1 && rect.width * width < 486.0
                || (zone == 2 || zone == 3) && rect.height * height < 326.0
            {
                return invalid_dock_target();
            }
            let mut preview = rect;
            match zone {
                0 => preview.width *= 0.5,
                1 => {
                    preview.x += preview.width * 0.5;
                    preview.width *= 0.5;
                }
                2 => preview.height *= 0.5,
                3 => {
                    preview.y += preview.height * 0.5;
                    preview.height *= 0.5;
                }
                _ => {}
            }
            DockTarget {
                group: group as i32,
                zone,
                x: preview.x,
                y: preview.y,
                width: preview.width,
                height: preview.height,
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_pane_dock_requested(move |tab_id, target, zone| {
            let Some(ui) = weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            let (Ok(id), Ok(target), Some(dock)) = (
                TabId::try_from(tab_id),
                usize::try_from(target),
                Dock::from_zone(zone),
            ) else {
                return;
            };
            cancel_file_open(&mut state);
            if state.tab_groups.dock_into(id, target, dock) {
                state.maximized_group = None;
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_pane_divider_dragged(move |id, x, y| {
            let Some(ui) = weak.upgrade() else { return };
            let Ok(id) = usize::try_from(id) else { return };
            let mut state = state.borrow_mut();
            let (_, dividers) = state.tab_groups.layout();
            let Some(divider) = dividers.into_iter().find(|item| item.id == id) else {
                return;
            };
            let (local_x, local_y, width, height) = workspace_pointer(&ui, x, y);
            let (position, start, span, pixels, minimum) = match divider.axis {
                Axis::Horizontal => (
                    local_x,
                    divider.parent.x,
                    divider.parent.width,
                    width,
                    240.0,
                ),
                Axis::Vertical => (
                    local_y,
                    divider.parent.y,
                    divider.parent.height,
                    height,
                    160.0,
                ),
            };
            let minimum_fraction = ((minimum + 3.0) / (span * pixels).max(1.0)).min(0.5);
            let ratio = ((position - start) / span).clamp(minimum_fraction, 1.0 - minimum_fraction);
            if state
                .tab_groups
                .set_split_ratio(id, (ratio * 1000.0) as u16)
            {
                sync_layout(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_pane_maximize_requested(move |group| {
            let Some(ui) = weak.upgrade() else { return };
            let Ok(group) = usize::try_from(group) else {
                return;
            };
            let mut state = state.borrow_mut();
            if !state.tab_groups.groups().contains(&group) {
                return;
            }
            state.maximized_group = if state.maximized_group == Some(group) {
                None
            } else {
                Some(group)
            };
            sync_layout(&ui, &state);
        });
    }
    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_tree_target_requested(move |x, y| {
            let Some(ui) = weak.upgrade() else { return -1 };
            let state = state.borrow();
            let (x, y, _, _) = workspace_pointer(&ui, x, y);
            visible_panes(&state)
                .into_iter()
                .find_map(|(group, rect)| {
                    let id = state.tab_groups.active(group)?;
                    let is_terminal = matches!(
                        state
                            .tabs
                            .iter()
                            .find(|tab| tab.id == id)
                            .map(|tab| &tab.content),
                        Some(TabContent::Terminal { .. })
                    );
                    (is_terminal
                        && rect.contains(x, y)
                        && y >= rect.y + 36.0 / ui.get_workspace_area_height().max(1.0))
                    .then_some(group as i32)
                })
                .unwrap_or(-1)
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_pane_terminal_size_changed(move |group, rows, columns| {
            let Ok(group) = usize::try_from(group) else {
                return;
            };
            let mut state = state.borrow_mut();
            let size = (
                rows.round().clamp(2.0, u16::MAX as f32) as u16,
                columns.round().clamp(20.0, u16::MAX as f32) as u16,
            );
            if state.extra_terminal_sizes.insert(group, size) == Some(size) {
                return;
            }
            let Some(id) = state.tab_groups.active(group) else {
                return;
            };
            if terminal_mut(&mut state, id).is_some_and(|terminal| terminal.resize(size.0, size.1))
                && let Some(ui) = weak.upgrade()
            {
                sync_group(&ui, &state, group);
            }
        });
    }
    {
        let state = state.clone();
        ui.on_focused_group_changed(move |group| {
            state
                .borrow_mut()
                .tab_groups
                .set_focused_group(usize::try_from(group).unwrap_or(0));
        });
    }
    let loading_timer = Timer::default();
    {
        let weak = ui.as_weak();
        let state = state.clone();
        loading_timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            if let Some(result) = startup_loader.poll() {
                let mut focus_startup_terminal = false;
                match result {
                    Ok((monitor, terminal)) => {
                        if state.status == "Starting workspace..." {
                            state.status = "Ready".into();
                        }
                        match monitor {
                            Ok(monitor) => state.git_monitor = Some(monitor),
                            Err(error) => {
                                state.status = format!("Git auto-refresh unavailable: {error}")
                            }
                        }
                        match terminal {
                            Ok(session) => {
                                let tab_id = take_next_tab_id(&mut state);
                                connect_terminal_output(&ui, tab_id, &session);
                                let start_path = state.workspace.linux_root.clone();
                                let number = state.next_terminal_number;
                                state.next_terminal_number += 1;
                                state.tabs.push(WorkspaceTab {
                                    id: tab_id,
                                    content: TabContent::Terminal {
                                        session,
                                        start_path,
                                        number,
                                    },
                                });
                                focus_startup_terminal = state.tab_groups.add_background(tab_id, 0);
                            }
                            Err(error) => state.status = format!("Terminal unavailable: {error}"),
                        }
                    }
                    Err(error) => state.status = error.to_string(),
                }
                sync_ui(&ui, &state);
                if focus_startup_terminal {
                    ui.invoke_focus_terminal();
                }
            }
            if let Some(result) = state.tree_loader.poll() {
                match result {
                    Ok(tree) => {
                        if state.tree != tree {
                            state.tree = tree;
                            sync_tree(&ui, &state);
                        }
                    }
                    Err(error) => {
                        state.status = error.to_string();
                        ui.set_status_text(state.status.clone().into());
                    }
                }
            }
            if let Some(pixels) = icon_loader.poll() {
                state.emoji_icons = emoji::EmojiIcons::from_pixels(pixels);
                sync_tree(&ui, &state);
                sync_git_changes(&ui, &state);
            }
            if let Some(result) = state.file_loader.poll() {
                match result {
                    Ok((document, group)) => {
                        let tab_id = take_next_tab_id(&mut state);
                        state.tabs.push(WorkspaceTab {
                            id: tab_id,
                            content: TabContent::File(document),
                        });
                        state.tab_groups.add(tab_id, group);
                        state.status = "File opened".into();
                    }
                    Err(error) => state.status = error.to_string(),
                }
                sync_ui(&ui, &state);
            }
            if let Some(result) = state.diff_loader.poll() {
                match result {
                    Ok((diff, group)) => {
                        if state.statuses.contains_key(&diff.path) {
                            let tab_id = take_next_tab_id(&mut state);
                            state.tabs.push(WorkspaceTab {
                                id: tab_id,
                                content: TabContent::Diff(diff),
                            });
                            state.tab_groups.add(tab_id, group);
                            state.status = "Diff opened".into();
                        } else {
                            state.status = "Git change is no longer available".into();
                        }
                    }
                    Err(error) => state.status = error.to_string(),
                }
                sync_ui(&ui, &state);
            }
            if let Some(result) = state.git_action_loader.poll() {
                match result {
                    Ok(path) => {
                        close_diff_tabs(&mut state, &path);
                        state.status = format!("Discarded changes in {}", path.display());
                        if let Some(monitor) = state.git_monitor.as_mut() {
                            let _ = monitor.force_refresh();
                        } else {
                            state.statuses = git::read_status(&state.workspace);
                            refresh_tree(&mut state);
                        }
                    }
                    Err(error) => state.status = error.to_string(),
                }
                sync_ui(&ui, &state);
            }
            if let Some(result) = state.tree_action_loader.poll() {
                state.tree_action_pending = false;
                match result {
                    Ok(TreeActionResult::Created {
                        path,
                        parent,
                        is_directory,
                    }) => {
                        state.expanded.insert(parent);
                        refresh_tree(&mut state);
                        if let Some(monitor) = state.git_monitor.as_mut() {
                            let _ = monitor.force_refresh();
                        }
                        if is_directory {
                            state.status = format!("Created folder {}", path.display());
                        } else if let Err(error) = open_document(&mut state, path) {
                            state.status = error.to_string();
                        }
                    }
                    Ok(TreeActionResult::Deleted(path)) => {
                        close_file_tabs_at_or_below(&mut state, &path);
                        state
                            .expanded
                            .retain(|expanded| !expanded.starts_with(&path));
                        refresh_tree(&mut state);
                        if let Some(monitor) = state.git_monitor.as_mut() {
                            let _ = monitor.force_refresh();
                        }
                        state.status = format!("Deleted {}", path.display());
                    }
                    Ok(TreeActionResult::Renamed {
                        from,
                        to,
                        from_host,
                        to_host,
                    }) => {
                        state.file_loader.cancel();
                        state.diff_loader.cancel();
                        close_diff_tabs_at_or_below(&mut state, &from);
                        state.expanded = state
                            .expanded
                            .iter()
                            .map(|path| tree::rebased_path(path, &from, &to))
                            .collect();
                        state.expanded_git_repositories = state
                            .expanded_git_repositories
                            .iter()
                            .map(|path| tree::rebased_path(path, &from, &to))
                            .collect();
                        state.repositories = state
                            .repositories
                            .iter()
                            .map(|path| tree::rebased_path(path, &from, &to))
                            .collect();
                        for node in &mut state.tree {
                            if node.linux_path == from {
                                node.name = to
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned();
                            }
                            node.linux_path = tree::rebased_path(&node.linux_path, &from, &to);
                        }
                        for tab in &mut state.tabs {
                            if let TabContent::File(document) = &mut tab.content {
                                document.linux_path =
                                    tree::rebased_path(&document.linux_path, &from, &to);
                                document.host_path =
                                    tree::rebased_path(&document.host_path, &from_host, &to_host);
                            }
                        }
                        refresh_tree(&mut state);
                        if let Some(monitor) = state.git_monitor.as_mut() {
                            let _ = monitor.force_refresh();
                        }
                        state.status = format!("Renamed {} to {}", from.display(), to.display());
                    }
                    Ok(TreeActionResult::Copied { path, parent }) => {
                        state.expanded.insert(parent);
                        refresh_tree(&mut state);
                        if let Some(monitor) = state.git_monitor.as_mut() {
                            let _ = monitor.force_refresh();
                        }
                        state.status = format!("Copied {}", path.display());
                    }
                    Err(error) => state.status = error.to_string(),
                }
                sync_ui(&ui, &state);
            }
            for (group, view) in state.editor_views.borrow_mut().iter_mut().enumerate() {
                if let Some(highlighted) = view.poll(Instant::now()) {
                    let enabled = highlighted.is_some();
                    let text = highlighted.unwrap_or_default();
                    if group == 0 {
                        ui.set_highlighted_text(text);
                        ui.set_syntax_highlight_enabled(enabled);
                    } else {
                        ui.set_secondary_highlighted_text(text);
                        ui.set_secondary_syntax_highlight_enabled(enabled);
                    }
                }
            }
            for (&group, view) in state.extra_editor_views.borrow_mut().iter_mut() {
                if let Some(highlighted) = view.poll(Instant::now()) {
                    let enabled = highlighted.is_some();
                    let text = highlighted.unwrap_or_default();
                    edit_extra_pane(&ui, group, |pane| {
                        pane.highlighted_text = text;
                        pane.syntax_highlight_enabled = enabled;
                    });
                }
            }
        });
    }

    let timer = Timer::default();
    {
        let weak = ui.as_weak();
        let state = state.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
            let Some(ui) = weak.upgrade() else { return };
            let fixed_terminal_sizes = [
                (
                    ui.get_terminal_rows().round().clamp(2.0, u16::MAX as f32) as u16,
                    ui.get_terminal_columns()
                        .round()
                        .clamp(20.0, u16::MAX as f32) as u16,
                ),
                (
                    ui.get_secondary_terminal_rows()
                        .round()
                        .clamp(2.0, u16::MAX as f32) as u16,
                    ui.get_secondary_terminal_columns()
                        .round()
                        .clamp(20.0, u16::MAX as f32) as u16,
                ),
            ];
            let mut state = state.borrow_mut();
            let active = state
                .tab_groups
                .groups()
                .into_iter()
                .filter_map(|group| state.tab_groups.active(group).map(|id| (id, group)))
                .collect::<HashMap<_, _>>();
            let mut changed_groups = HashSet::new();
            let extra_sizes = state.extra_terminal_sizes.clone();
            for tab in &mut state.tabs {
                if let TabContent::Terminal { session, .. } = &mut tab.content {
                    let visible_group = active.get(&tab.id).copied();
                    let resized = visible_group.is_some_and(|group| {
                        let (rows, columns) = if group < 2 {
                            fixed_terminal_sizes[group]
                        } else {
                            extra_sizes.get(&group).copied().unwrap_or((24, 80))
                        };
                        session.resize(rows, columns)
                    });
                    if resized && let Some(group) = visible_group {
                        changed_groups.insert(group);
                    }
                }
            }
            for group in changed_groups {
                sync_group(&ui, &state, group);
            }
        });
    }

    let workspace_timer = Timer::default();
    {
        let weak = ui.as_weak();
        let state = state.clone();
        workspace_timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            let update = state
                .git_monitor
                .as_mut()
                .and_then(git::StatusMonitor::poll_latest);
            let Some(update) = update else {
                return;
            };
            let snapshot = match update {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    state.status = format!("Git auto-refresh failed: {error}");
                    ui.set_status_text(state.status.clone().into());
                    return;
                }
            };
            let previous_tree = state.tree.clone();
            let previous_statuses = state.statuses.clone();
            let previous_repositories = state.repositories.clone();
            let previous_status = state.status.clone();
            let previous_save_conflict = state.save_conflict;
            state.statuses = snapshot.statuses;
            state.repositories = snapshot.repositories;
            if state.status == "Refreshing..." {
                state.status = "Refreshed".into();
            }
            refresh_tree(&mut state);
            let refreshed_groups = refresh_external_documents(&mut state);
            if state.tree != previous_tree
                || state.statuses != previous_statuses
                || state.repositories != previous_repositories
            {
                sync_tree(&ui, &state);
                sync_git_changes(&ui, &state);
            }
            for group in refreshed_groups {
                sync_group(&ui, &state, group);
            }
            if state.status != previous_status {
                ui.set_status_text(state.status.clone().into());
            }
            if state.save_conflict != previous_save_conflict {
                ui.set_save_conflict(state.save_conflict.is_some());
            }
        });
    }

    ui.run()?;
    drop(timer);
    drop(workspace_timer);
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_windows_file_drop(ui: &AppWindow, state: Rc<RefCell<AppState>>) {
    use slint::winit_030::{EventResult, winit};

    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |window, event| {
        if let winit::event::WindowEvent::DroppedFile(source) = event
            && let Some(ui) = weak.upgrade()
            && let Some((x, y)) = windows_cursor_position(window)
        {
            begin_external_file_copy(&ui, &state, source.clone(), x, y);
        }
        EventResult::Propagate
    });
}

#[cfg(not(target_os = "windows"))]
fn install_windows_file_drop(_ui: &AppWindow, _state: Rc<RefCell<AppState>>) {}

#[cfg(target_os = "windows")]
fn windows_cursor_position(window: &slint::Window) -> Option<(f32, f32)> {
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    window
        .with_winit_window(|window| {
            let handle = window.window_handle().ok()?;
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return None;
            };
            let hwnd = handle.hwnd.get() as HWND;
            let mut point = POINT { x: 0, y: 0 };
            if unsafe { GetCursorPos(&mut point) } == 0
                || unsafe { ScreenToClient(hwnd, &mut point) } == 0
            {
                return None;
            }
            let scale = window.scale_factor() as f32;
            Some((point.x as f32 / scale, point.y as f32 / scale))
        })
        .flatten()
}

#[cfg(target_os = "windows")]
fn begin_external_file_copy(
    ui: &AppWindow,
    state: &Rc<RefCell<AppState>>,
    source: PathBuf,
    x: f32,
    y: f32,
) {
    let mut state = state.borrow_mut();
    let Some(parent) = external_file_drop_parent(ui, &state, x, y) else {
        return;
    };
    if state.tree_action_pending {
        state.status = "A file tree action is already running".into();
    } else {
        let workspace = state.workspace.clone();
        let result_parent = parent.clone();
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.display().to_string());
        state.tree_action_pending = true;
        state.status = format!("Copying {name} to {}...", parent.display());
        state.tree_action_loader.request(move || {
            let path = tree::copy_external_file(&workspace, &parent, &source)?;
            Ok(TreeActionResult::Copied {
                path,
                parent: result_parent,
            })
        });
    }
    sync_ui(ui, &state);
}

#[cfg(target_os = "windows")]
fn external_file_drop_parent(ui: &AppWindow, state: &AppState, x: f32, y: f32) -> Option<PathBuf> {
    if ui.get_sidebar_view() != 0 {
        return None;
    }
    let left = ui.get_file_tree_x();
    let top = ui.get_file_tree_y();
    let width = ui.get_file_tree_width();
    let height = ui.get_file_tree_height();
    if x < left || x >= left + width || y < top || y >= top + height {
        return None;
    }
    let content_y = y - top - ui.get_file_tree_scroll_offset();
    tree::external_drop_target(
        &state.tree,
        &state.workspace.linux_root,
        content_y,
        ui.get_file_tree_row_height(),
    )
}

fn parse_args() -> Result<(String, PathBuf, Option<PathBuf>)> {
    let mut args = std::env::args().skip(1);
    let mut distro = std::env::var("WSL_DISTRO_NAME").unwrap_or_else(|_| "Ubuntu".into());
    let mut root: Option<PathBuf> = None;
    let mut file = None;
    let mut internal_root = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--distro" => distro = args.next().context("--distro requires a value")?,
            "--root" => {
                root = Some(PathBuf::from(
                    args.next().context("--root requires a value")?,
                ));
                internal_root = true;
            }
            "--file" => {
                file = Some(PathBuf::from(
                    args.next().context("--file requires a value")?,
                ))
            }
            "--help" | "-h" => {
                println!(
                    "Araseo {}\nUsage: araseo [PATH]\n       araseo.exe --distro NAME --root LINUX_PATH [--file FILE]",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("araseo {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            value if value.starts_with('-') => bail!("unknown option: {value}"),
            value => root = Some(PathBuf::from(value)),
        }
    }
    let supplied = root.unwrap_or(std::env::current_dir()?);
    let supplied = if internal_root {
        supplied
    } else {
        supplied
            .canonicalize()
            .with_context(|| format!("cannot resolve path: {}", supplied.display()))?
    };
    let root = if supplied.is_file() {
        file = Some(supplied.clone());
        supplied
            .parent()
            .context("file does not have a parent directory")?
            .to_path_buf()
    } else {
        supplied
    };
    Ok((distro, root, file))
}

fn apply_history_change(
    weak: &slint::Weak<AppWindow>,
    state: &Rc<RefCell<AppState>>,
    tab_id: i32,
    undo: bool,
) {
    let Ok(tab_id) = TabId::try_from(tab_id) else {
        return;
    };
    let mut state = state.borrow_mut();
    let is_active = state
        .tab_groups
        .group_of(tab_id)
        .is_some_and(|group| state.tab_groups.active(group) == Some(tab_id));
    if !is_active {
        return;
    }
    let cursor = document_mut(&mut state, tab_id).and_then(|document| {
        if undo {
            document.undo()
        } else {
            document.redo()
        }
    });
    let Some(cursor) = cursor else {
        return;
    };
    if let Some(ui) = weak.upgrade() {
        if let Some(group) = state.tab_groups.group_of(tab_id) {
            state.syncing_editor.set(true);
            sync_group(&ui, &state, group);
            if group == 0 {
                ui.set_editor_cursor_offset(cursor.min(i32::MAX as usize) as i32);
                ui.set_editor_cursor_generation(ui.get_editor_cursor_generation().wrapping_add(1));
            } else if group == 1 {
                ui.set_secondary_editor_cursor_offset(cursor.min(i32::MAX as usize) as i32);
                ui.set_secondary_editor_cursor_generation(
                    ui.get_secondary_editor_cursor_generation().wrapping_add(1),
                );
            } else {
                edit_extra_pane(&ui, group, |pane| {
                    pane.editor_cursor_offset = cursor.min(i32::MAX as usize) as i32;
                    pane.editor_cursor_generation = pane.editor_cursor_generation.wrapping_add(1);
                });
            }
            state.syncing_editor.set(false);
            sync_tabs(&ui, &state);
        }
    }
}

fn cancel_file_open(state: &mut AppState) {
    state.file_loader.cancel();
    state.diff_loader.cancel();
    if state.status.starts_with("Opening ") || state.status.starts_with("Loading diff for ") {
        state.status = "Ready".into();
    }
}

fn open_document(state: &mut AppState, linux_path: PathBuf) -> Result<()> {
    cancel_file_open(state);
    if let Some(tab_id) = state.tabs.iter().find_map(|tab| match &tab.content {
        TabContent::File(document) if document.linux_path == linux_path => Some(tab.id),
        _ => None,
    }) {
        state.tab_groups.activate(tab_id);
        return Ok(());
    }

    let workspace = state.workspace.clone();
    let group = state.tab_groups.focused_group();
    state.status = format!("Opening {}...", linux_path.display());
    state.file_loader.request(move || {
        let host_path = workspace.host_path(&linux_path)?;
        Ok((Document::open(linux_path, host_path)?, group))
    });
    Ok(())
}

fn open_git_diff(state: &mut AppState, path: PathBuf) -> Result<()> {
    cancel_file_open(state);
    if let Some(tab_id) = state.tabs.iter().find_map(|tab| match &tab.content {
        TabContent::Diff(diff) if diff.path == path => Some(tab.id),
        _ => None,
    }) {
        state.tab_groups.activate(tab_id);
        return Ok(());
    }

    let status = state
        .statuses
        .get(&path)
        .copied()
        .with_context(|| format!("Git change is no longer available: {}", path.display()))?;
    let repository = repository_for(&state.repositories, &path)
        .with_context(|| format!("Cannot find repository for {}", path.display()))?;
    let workspace = state.workspace.clone();
    let group = state.tab_groups.focused_group();
    state.status = format!("Loading diff for {}...", path.display());
    state.diff_loader.request(move || {
        Ok((
            git::load_diff(&workspace, &repository, &path, status)?,
            group,
        ))
    });
    Ok(())
}

fn open_terminal(state: &mut AppState, group: usize) -> Result<TabId> {
    cancel_file_open(state);
    let session = TerminalSession::spawn(&state.workspace.distro, &state.workspace.linux_root)?;
    let tab_id = take_next_tab_id(state);
    let number = state.next_terminal_number;
    state.next_terminal_number = state.next_terminal_number.saturating_add(1);
    state.tabs.push(WorkspaceTab {
        id: tab_id,
        content: TabContent::Terminal {
            session,
            start_path: state.workspace.linux_root.clone(),
            number,
        },
    });
    state.tab_groups.add(tab_id, group);
    state.status = "Terminal opened".into();
    Ok(tab_id)
}

fn close_tab(state: &mut AppState, tab_id: TabId) {
    cancel_file_open(state);
    let Some(index) = state.tabs.iter().position(|tab| tab.id == tab_id) else {
        return;
    };
    if matches!(&state.tabs[index].content, TabContent::File(document) if document.dirty) {
        state.status = "Save the modified file before closing it".into();
        return;
    }

    state.tab_groups.remove(tab_id);
    state.tabs.remove(index);
    if state.save_conflict == Some(tab_id) {
        state.save_conflict = None;
    }
    state.status = "Tab closed".into();
}

fn close_diff_tabs(state: &mut AppState, path: &std::path::Path) {
    let tab_ids = state
        .tabs
        .iter()
        .filter_map(|tab| match &tab.content {
            TabContent::Diff(diff) if diff.path == path => Some(tab.id),
            _ => None,
        })
        .collect::<Vec<_>>();
    for tab_id in tab_ids {
        state.tab_groups.remove(tab_id);
        if let Some(index) = state.tabs.iter().position(|tab| tab.id == tab_id) {
            state.tabs.remove(index);
        }
    }
}

fn close_diff_tabs_at_or_below(state: &mut AppState, path: &std::path::Path) {
    let tab_ids = state
        .tabs
        .iter()
        .filter_map(|tab| match &tab.content {
            TabContent::Diff(diff) if diff.path.starts_with(path) => Some(tab.id),
            _ => None,
        })
        .collect::<Vec<_>>();
    for tab_id in tab_ids {
        state.tab_groups.remove(tab_id);
        if let Some(index) = state.tabs.iter().position(|tab| tab.id == tab_id) {
            state.tabs.remove(index);
        }
    }
}

fn tree_node_for_ui_path(state: &AppState, path: &str) -> Option<FlatNode> {
    state
        .tree
        .iter()
        .find(|node| tree::linux_path_text(&node.linux_path) == path)
        .cloned()
}

fn has_dirty_document_at_or_below(state: &AppState, path: &std::path::Path) -> bool {
    state.tabs.iter().any(|tab| {
        matches!(
            &tab.content,
            TabContent::File(document)
                if document.dirty && document.linux_path.starts_with(path)
        )
    })
}

fn close_file_tabs_at_or_below(state: &mut AppState, path: &std::path::Path) {
    let tab_ids = state
        .tabs
        .iter()
        .filter_map(|tab| {
            let matches = match &tab.content {
                TabContent::File(document) => document.linux_path.starts_with(path),
                TabContent::Diff(diff) => diff.path.starts_with(path),
                TabContent::Terminal { .. } => false,
            };
            matches.then_some(tab.id)
        })
        .collect::<Vec<_>>();
    for tab_id in tab_ids {
        state.tab_groups.remove(tab_id);
        if let Some(index) = state.tabs.iter().position(|tab| tab.id == tab_id) {
            state.tabs.remove(index);
        }
        if state.save_conflict == Some(tab_id) {
            state.save_conflict = None;
        }
    }
}

fn save_tab(state: &mut AppState, tab_id: TabId, overwrite_external: bool) {
    let Some(document) = document_mut(state, tab_id) else {
        return;
    };
    match document.save(overwrite_external) {
        Ok(()) => {
            state.status = "Saved".into();
            state.save_conflict = None;
            if let Some(monitor) = state.git_monitor.as_mut() {
                let _ = monitor.force_refresh();
            } else {
                state.statuses = git::read_status(&state.workspace);
            }
            refresh_tree(state);
        }
        Err(error) => {
            state.save_conflict = document_ref(state, tab_id)
                .is_some_and(Document::changed_on_disk)
                .then_some(tab_id);
            state.status = error.to_string();
        }
    }
}

fn reload_tab(state: &mut AppState, tab_id: TabId) {
    let Some(document) = document_ref(state, tab_id) else {
        return;
    };
    let linux_path = document.linux_path.clone();
    let host_path = document.host_path.clone();
    match Document::open(linux_path, host_path) {
        Ok(document) => {
            if let Some(target) = document_mut(state, tab_id) {
                *target = document;
            }
            state.save_conflict = None;
            state.status = "Reloaded from disk".into();
        }
        Err(error) => state.status = error.to_string(),
    }
}

fn refresh_external_documents(state: &mut AppState) -> HashSet<usize> {
    let active = state
        .tab_groups
        .groups()
        .into_iter()
        .filter_map(|group| state.tab_groups.active(group).map(|id| (id, group)))
        .collect::<HashMap<_, _>>();
    let mut refreshed_groups = HashSet::new();
    let mut reloaded = 0usize;
    let mut conflict = None;
    let mut refresh_error = None;
    let mut resolved_conflict = false;

    for tab in &mut state.tabs {
        let TabContent::File(document) = &mut tab.content else {
            continue;
        };
        let path = document.linux_path.clone();
        match document.refresh_from_disk() {
            Ok(ExternalRefresh::Unchanged) => {}
            Ok(ExternalRefresh::Reloaded) => {
                reloaded += 1;
                resolved_conflict |= state.save_conflict == Some(tab.id);
                if let Some(group) = active.get(&tab.id) {
                    refreshed_groups.insert(*group);
                }
            }
            Ok(ExternalRefresh::Conflict) => {
                if conflict.is_none() {
                    conflict = Some((tab.id, path));
                }
            }
            Err(error) => {
                if refresh_error.is_none() {
                    refresh_error = Some(format!("Cannot refresh {}: {error}", path.display()));
                }
            }
        }
    }

    if resolved_conflict {
        state.save_conflict = None;
    }
    if let Some((tab_id, path)) = conflict {
        state.save_conflict = Some(tab_id);
        state.status = format!(
            "{} changed outside Araseo; reload or overwrite",
            path.display()
        );
    } else if let Some(error) = refresh_error {
        state.status = error;
    } else if reloaded == 1 {
        state.status = "Updated open file from disk".into();
    } else if reloaded > 1 {
        state.status = format!("Updated {reloaded} open files from disk");
    }

    refreshed_groups
}

fn take_next_tab_id(state: &mut AppState) -> TabId {
    let id = state.next_tab_id;
    state.next_tab_id = state.next_tab_id.wrapping_add(1);
    id
}

fn focused_tab_id(state: &AppState) -> Option<TabId> {
    state.tab_groups.active(state.tab_groups.focused_group())
}

fn repository_for(repositories: &HashSet<PathBuf>, path: &std::path::Path) -> Option<PathBuf> {
    repositories
        .iter()
        .filter(|repository| path.starts_with(repository))
        .max_by_key(|repository| repository.components().count())
        .cloned()
}

fn document_ref(state: &AppState, tab_id: TabId) -> Option<&Document> {
    state.tabs.iter().find_map(|tab| {
        if tab.id != tab_id {
            return None;
        }
        match &tab.content {
            TabContent::File(document) => Some(document),
            TabContent::Diff(_) | TabContent::Terminal { .. } => None,
        }
    })
}

fn document_mut(state: &mut AppState, tab_id: TabId) -> Option<&mut Document> {
    state.tabs.iter_mut().find_map(|tab| {
        if tab.id != tab_id {
            return None;
        }
        match &mut tab.content {
            TabContent::File(document) => Some(document),
            TabContent::Diff(_) | TabContent::Terminal { .. } => None,
        }
    })
}

fn terminal_ref(state: &AppState, tab_id: TabId) -> Option<&TerminalSession> {
    state.tabs.iter().find_map(|tab| {
        if tab.id != tab_id {
            return None;
        }
        match &tab.content {
            TabContent::Terminal { session, .. } => Some(session),
            TabContent::File(_) | TabContent::Diff(_) => None,
        }
    })
}

fn terminal_mut(state: &mut AppState, tab_id: TabId) -> Option<&mut TerminalSession> {
    state.tabs.iter_mut().find_map(|tab| {
        if tab.id != tab_id {
            return None;
        }
        match &mut tab.content {
            TabContent::Terminal { session, .. } => Some(session),
            TabContent::File(_) | TabContent::Diff(_) => None,
        }
    })
}

fn connect_terminal_output(ui: &AppWindow, tab_id: TabId, terminal: &TerminalSession) {
    let weak = ui.as_weak();
    terminal.set_output_waker(move || {
        let weak = weak.clone();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.invoke_terminal_output_ready(tab_id as i32);
        });
    });
}

fn refresh_tree(state: &mut AppState) {
    let workspace = state.workspace.clone();
    let expanded = state.expanded.clone();
    let statuses = state.statuses.clone();
    state
        .tree_loader
        .request(move || tree::build_tree(&workspace, &expanded, &statuses));
}

fn workspace_pointer(ui: &AppWindow, x: f32, y: f32) -> (f32, f32, f32, f32) {
    let width = ui.get_workspace_area_width().max(1.0);
    let height = ui.get_workspace_area_height().max(1.0);
    (
        (x - ui.get_workspace_area_x()) / width,
        (y - ui.get_workspace_area_y()) / height,
        width,
        height,
    )
}

fn invalid_dock_target() -> DockTarget {
    DockTarget {
        group: -1,
        zone: -1,
        ..DockTarget::default()
    }
}

fn visible_panes(state: &AppState) -> Vec<(usize, Rect)> {
    if let Some(group) = state
        .maximized_group
        .filter(|group| state.tab_groups.groups().contains(group))
    {
        return vec![(
            group,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
        )];
    }
    state.tab_groups.layout().0
}

fn edit_extra_pane(ui: &AppWindow, group: usize, edit: impl FnOnce(&mut PaneEntry)) {
    let panes = ui.get_extra_panes();
    let model = panes
        .as_any()
        .downcast_ref::<VecModel<PaneEntry>>()
        .expect("extra panes use a VecModel");
    let index = group - 2;
    while model.row_count() <= index {
        let mut pane = PaneEntry::default();
        pane.group = (model.row_count() + 2) as i32;
        model.push(pane);
    }
    let mut pane = model.row_data(index).unwrap();
    edit(&mut pane);
    model.set_row_data(index, pane);
}

fn extra_pane(ui: &AppWindow, group: usize) -> Option<PaneEntry> {
    ui.get_extra_panes().row_data(group.checked_sub(2)?)
}

fn sync_layout(ui: &AppWindow, state: &AppState) {
    let maximized = state
        .maximized_group
        .filter(|group| state.tab_groups.groups().contains(group));
    ui.set_maximized_pane(maximized.map_or(-1, |group| group as i32));
    let layout = visible_panes(state).into_iter().collect::<HashMap<_, _>>();
    ui.set_primary_layout_visible(layout.contains_key(&0));
    ui.set_secondary_layout_visible(layout.contains_key(&1));
    if let Some(rect) = layout.get(&0) {
        ui.set_primary_layout_x(rect.x);
        ui.set_primary_layout_y(rect.y);
        ui.set_primary_layout_width(rect.width);
        ui.set_primary_layout_height(rect.height);
    }
    if let Some(rect) = layout.get(&1) {
        ui.set_secondary_layout_x(rect.x);
        ui.set_secondary_layout_y(rect.y);
        ui.set_secondary_layout_width(rect.width);
        ui.set_secondary_layout_height(rect.height);
    }
    let panes = ui.get_extra_panes();
    let model = panes
        .as_any()
        .downcast_ref::<VecModel<PaneEntry>>()
        .expect("extra panes use a VecModel");
    for group in layout.keys().copied().filter(|group| *group >= 2) {
        while model.row_count() <= group - 2 {
            let mut pane = PaneEntry::default();
            pane.group = (model.row_count() + 2) as i32;
            model.push(pane);
        }
    }
    for index in 0..model.row_count() {
        let mut pane = model.row_data(index).unwrap();
        let rect = layout.get(&(index + 2));
        let visible = rect.is_some();
        let changed = pane.visible != visible
            || rect.is_some_and(|rect| {
                pane.x != rect.x
                    || pane.y != rect.y
                    || pane.width != rect.width
                    || pane.height != rect.height
            });
        if changed {
            pane.visible = visible;
            if let Some(rect) = rect {
                pane.x = rect.x;
                pane.y = rect.y;
                pane.width = rect.width;
                pane.height = rect.height;
            }
            model.set_row_data(index, pane);
        }
    }
    let dividers = if maximized.is_some() {
        Vec::new()
    } else {
        state
            .tab_groups
            .layout()
            .1
            .into_iter()
            .map(|divider| DividerEntry {
                id: divider.id as i32,
                horizontal: divider.axis == Axis::Horizontal,
                x: divider.rect.x,
                y: divider.rect.y,
                width: divider.rect.width,
                height: divider.rect.height,
            })
            .collect::<Vec<_>>()
    };
    let current = ui.get_pane_dividers();
    if let Some(model) = current.as_any().downcast_ref::<VecModel<DividerEntry>>()
        && model.row_count() == dividers.len()
        && dividers.iter().enumerate().all(|(index, divider)| {
            model
                .row_data(index)
                .is_some_and(|current| current.id == divider.id)
        })
    {
        for (index, divider) in dividers.into_iter().enumerate() {
            model.set_row_data(index, divider);
        }
    } else {
        ui.set_pane_dividers(ModelRc::new(VecModel::from(dividers)));
    }
}

fn sync_ui(ui: &AppWindow, state: &AppState) {
    ui.set_workspace_name(
        state
            .workspace
            .linux_root
            .to_string_lossy()
            .to_string()
            .into(),
    );
    ui.set_status_text(state.status.clone().into());
    ui.set_save_conflict(state.save_conflict.is_some());
    ui.set_tree_action_pending(state.tree_action_pending);
    ui.set_focused_group(state.tab_groups.focused_group() as i32);
    sync_layout(ui, state);
    sync_tree(ui, state);
    sync_git_changes(ui, state);
    sync_tabs(ui, state);
    for group in state.tab_groups.groups() {
        sync_group(ui, state, group);
    }

    let active_path = focused_tab_id(state)
        .and_then(|tab_id| state.tabs.iter().find(|tab| tab.id == tab_id))
        .map(tab_detail)
        .unwrap_or_default();
    ui.set_active_path(active_path.into());
}

fn sync_group(ui: &AppWindow, state: &AppState, group: usize) {
    if group >= 2 {
        sync_extra_group(ui, state, group);
        return;
    }
    let active_id = state.tab_groups.active(group);
    let active_tab = active_id.and_then(|id| state.tabs.iter().find(|tab| tab.id == id));

    match (group, active_tab) {
        (0, Some(tab)) => {
            ui.set_primary_active_tab_id(tab.id as i32);
            ui.set_primary_active_title(tab_title(tab).into());
            ui.set_primary_active_detail(tab_detail(tab).into());
            match &tab.content {
                TabContent::File(document) => {
                    ui.set_primary_active_kind("file".into());
                    state.syncing_editor.set(true);
                    if ui.get_editor_text().as_str() != document.text {
                        ui.set_editor_text(document.text.clone().into());
                    }
                    state.syncing_editor.set(false);
                    sync_editor_view(ui, state, document, 0, ui.get_editor_text());
                    clear_terminal_group(ui, 0);
                }
                TabContent::Diff(diff) => {
                    state.editor_views.borrow_mut()[0].clear();
                    ui.set_primary_active_kind("diff".into());
                    ui.set_syntax_highlight_enabled(false);
                    ui.set_highlighted_text(slint::StyledText::default());
                    sync_diff(ui, diff, 0);
                    clear_terminal_group(ui, 0);
                }
                TabContent::Terminal { session, .. } => {
                    state.editor_views.borrow_mut()[0].clear();
                    ui.set_primary_active_kind("terminal".into());
                    ui.set_syntax_highlight_enabled(false);
                    ui.set_highlighted_text(slint::StyledText::default());
                    sync_terminal(ui, session, 0);
                }
            }
        }
        (1, Some(tab)) => {
            ui.set_secondary_active_tab_id(tab.id as i32);
            ui.set_secondary_active_title(tab_title(tab).into());
            ui.set_secondary_active_detail(tab_detail(tab).into());
            match &tab.content {
                TabContent::File(document) => {
                    ui.set_secondary_active_kind("file".into());
                    state.syncing_editor.set(true);
                    if ui.get_secondary_editor_text().as_str() != document.text {
                        ui.set_secondary_editor_text(document.text.clone().into());
                    }
                    state.syncing_editor.set(false);
                    sync_editor_view(ui, state, document, 1, ui.get_secondary_editor_text());
                    clear_terminal_group(ui, 1);
                }
                TabContent::Diff(diff) => {
                    state.editor_views.borrow_mut()[1].clear();
                    ui.set_secondary_active_kind("diff".into());
                    ui.set_secondary_syntax_highlight_enabled(false);
                    ui.set_secondary_highlighted_text(slint::StyledText::default());
                    sync_diff(ui, diff, 1);
                    clear_terminal_group(ui, 1);
                }
                TabContent::Terminal { session, .. } => {
                    state.editor_views.borrow_mut()[1].clear();
                    ui.set_secondary_active_kind("terminal".into());
                    ui.set_secondary_syntax_highlight_enabled(false);
                    ui.set_secondary_highlighted_text(slint::StyledText::default());
                    sync_terminal(ui, session, 1);
                }
            }
        }
        (0, None) => {
            state.editor_views.borrow_mut()[0].clear();
            ui.set_primary_active_tab_id(-1);
            ui.set_primary_active_kind("".into());
            ui.set_primary_active_title("".into());
            ui.set_primary_active_detail("".into());
            ui.set_editor_text("".into());
            ui.set_line_numbers("1".into());
            ui.set_syntax_highlight_enabled(false);
            ui.set_highlighted_text(slint::StyledText::default());
            ui.set_diff_rows(ModelRc::new(VecModel::from(Vec::<DiffRow>::new())));
            clear_terminal_group(ui, 0);
        }
        (1, None) => {
            state.editor_views.borrow_mut()[1].clear();
            ui.set_secondary_active_tab_id(-1);
            ui.set_secondary_active_kind("".into());
            ui.set_secondary_active_title("".into());
            ui.set_secondary_active_detail("".into());
            ui.set_secondary_editor_text("".into());
            ui.set_secondary_line_numbers("1".into());
            ui.set_secondary_syntax_highlight_enabled(false);
            ui.set_secondary_highlighted_text(slint::StyledText::default());
            ui.set_secondary_diff_rows(ModelRc::new(VecModel::from(Vec::<DiffRow>::new())));
            clear_terminal_group(ui, 1);
        }
        _ => {}
    }
}

fn sync_extra_group(ui: &AppWindow, state: &AppState, group: usize) {
    let active = state
        .tab_groups
        .active(group)
        .and_then(|id| state.tabs.iter().find(|tab| tab.id == id));
    let Some(tab) = active else {
        state.extra_editor_views.borrow_mut().remove(&group);
        edit_extra_pane(ui, group, |pane| {
            pane.active_tab_id = -1;
            pane.active_kind = "".into();
            pane.active_title = "".into();
            pane.active_detail = "".into();
            pane.editor_text = "".into();
            pane.highlighted_text = slint::StyledText::default();
            pane.syntax_highlight_enabled = false;
            pane.diff_rows = ModelRc::new(VecModel::from(Vec::<DiffRow>::new()));
        });
        clear_terminal_group(ui, group);
        return;
    };
    edit_extra_pane(ui, group, |pane| {
        pane.active_tab_id = tab.id as i32;
        pane.active_title = tab_title(tab).into();
        pane.active_detail = tab_detail(tab).into();
        pane.active_kind = match tab.content {
            TabContent::File(_) => "file",
            TabContent::Diff(_) => "diff",
            TabContent::Terminal { .. } => "terminal",
        }
        .into();
    });
    match &tab.content {
        TabContent::File(document) => {
            state.syncing_editor.set(true);
            edit_extra_pane(ui, group, |pane| {
                if pane.editor_text.as_str() != document.text {
                    pane.editor_text = document.text.clone().into();
                }
            });
            state.syncing_editor.set(false);
            if let Some(pane) = extra_pane(ui, group) {
                sync_editor_view(ui, state, document, group, pane.editor_text);
            }
            clear_terminal_group(ui, group);
        }
        TabContent::Diff(diff) => {
            state.extra_editor_views.borrow_mut().remove(&group);
            edit_extra_pane(ui, group, |pane| {
                pane.syntax_highlight_enabled = false;
                pane.highlighted_text = slint::StyledText::default();
            });
            sync_diff(ui, diff, group);
            clear_terminal_group(ui, group);
        }
        TabContent::Terminal { session, .. } => {
            state.extra_editor_views.borrow_mut().remove(&group);
            edit_extra_pane(ui, group, |pane| {
                pane.syntax_highlight_enabled = false;
                pane.highlighted_text = slint::StyledText::default();
            });
            sync_terminal(ui, session, group);
        }
    }
}

fn sync_editor_view(
    ui: &AppWindow,
    state: &AppState,
    document: &Document,
    group: usize,
    text: slint::SharedString,
) {
    let (changed, numbers) = if group < 2 {
        state.editor_views.borrow_mut()[group].update(
            &document.linux_path,
            text,
            ui.get_editor_font_brightness(),
            Instant::now(),
        )
    } else {
        state
            .extra_editor_views
            .borrow_mut()
            .entry(group)
            .or_default()
            .update(
                &document.linux_path,
                text,
                ui.get_editor_font_brightness(),
                Instant::now(),
            )
    };
    if !changed {
        return;
    }
    match group {
        0 => {
            if let Some(numbers) = numbers {
                ui.set_line_numbers(numbers);
            }
            ui.set_highlighted_text(slint::StyledText::default());
            ui.set_syntax_highlight_enabled(false);
        }
        1 => {
            if let Some(numbers) = numbers {
                ui.set_secondary_line_numbers(numbers);
            }
            ui.set_secondary_highlighted_text(slint::StyledText::default());
            ui.set_secondary_syntax_highlight_enabled(false);
        }
        _ => edit_extra_pane(ui, group, |pane| {
            if let Some(numbers) = numbers {
                pane.line_numbers = numbers;
            }
            pane.highlighted_text = slint::StyledText::default();
            pane.syntax_highlight_enabled = false;
        }),
    }
}

fn sync_diff(ui: &AppWindow, diff: &git::FileDiff, group: usize) {
    let old_lines = diff
        .lines
        .iter()
        .map(|line| line.old_number.map(|_| line.old_text.as_str()))
        .collect::<Vec<_>>();
    let new_lines = diff
        .lines
        .iter()
        .map(|line| line.new_number.map(|_| line.new_text.as_str()))
        .collect::<Vec<_>>();
    let brightness = ui.get_editor_font_brightness();
    let old_markup = highlight::line_markup_with_brightness(&diff.path, &old_lines, brightness);
    let new_markup = highlight::line_markup_with_brightness(&diff.path, &new_lines, brightness);
    let rows = diff
        .lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let old_highlighted = old_markup
                .as_ref()
                .and_then(|lines| lines[index].as_ref())
                .and_then(|markup| slint::StyledText::from_markdown(markup).ok());
            let new_highlighted = new_markup
                .as_ref()
                .and_then(|lines| lines[index].as_ref())
                .and_then(|markup| slint::StyledText::from_markdown(markup).ok());
            DiffRow {
                old_line: line
                    .old_number
                    .map(|number| number.to_string())
                    .unwrap_or_default()
                    .into(),
                new_line: line
                    .new_number
                    .map(|number| number.to_string())
                    .unwrap_or_default()
                    .into(),
                old_text: line.old_text.clone().into(),
                new_text: line.new_text.clone().into(),
                old_highlighted_enabled: old_highlighted.is_some(),
                new_highlighted_enabled: new_highlighted.is_some(),
                old_highlighted: old_highlighted.unwrap_or_default(),
                new_highlighted: new_highlighted.unwrap_or_default(),
                old_kind: diff_kind_name(line.old_kind).into(),
                new_kind: diff_kind_name(line.new_kind).into(),
            }
        })
        .collect::<Vec<_>>();
    let relative = diff
        .path
        .strip_prefix(&diff.repository)
        .unwrap_or(&diff.path);
    let relative = relative.to_string_lossy();
    let old_title = format!("{relative} (HEAD)");
    let new_title = match diff.status {
        GitStatus::Deleted => format!("{relative} (Deleted)"),
        _ => format!("{relative} (Working Tree)"),
    };
    if group == 0 {
        ui.set_diff_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_diff_old_title(old_title.into());
        ui.set_diff_new_title(new_title.into());
    } else if group == 1 {
        ui.set_secondary_diff_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_secondary_diff_old_title(old_title.into());
        ui.set_secondary_diff_new_title(new_title.into());
    } else {
        edit_extra_pane(ui, group, |pane| {
            pane.diff_rows = ModelRc::new(VecModel::from(rows));
            pane.diff_old_title = old_title.into();
            pane.diff_new_title = new_title.into();
        });
    }
}

fn diff_kind_name(kind: git::DiffKind) -> &'static str {
    match kind {
        git::DiffKind::Context => "context",
        git::DiffKind::Removed => "removed",
        git::DiffKind::Added => "added",
        git::DiffKind::Empty => "empty",
    }
}

fn sync_terminal(ui: &AppWindow, terminal: &TerminalSession, group: usize) {
    let (rows, columns) = terminal.size();
    let cells = terminal
        .cells()
        .into_iter()
        .map(|cell| TerminalCell {
            row: cell.row,
            column: cell.column,
            glyph: cell.glyph.into(),
            foreground: slint::Color::from_rgb_u8(
                cell.foreground[0],
                cell.foreground[1],
                cell.foreground[2],
            ),
            background: slint::Color::from_rgb_u8(
                cell.background[0],
                cell.background[1],
                cell.background[2],
            ),
            bold: cell.bold,
            cursor: cell.cursor,
            column_span: cell.column_span,
        })
        .collect::<Vec<_>>();
    if group == 0 {
        ui.set_terminal_grid_rows(rows.into());
        ui.set_terminal_grid_columns(columns.into());
        ui.set_terminal_cursor_row(terminal.cursor_row());
        ui.set_terminal_cursor_column(terminal.cursor_column());
        update_terminal_model(ui.get_terminal_cells(), cells, |model| {
            ui.set_terminal_cells(model)
        });
        ui.set_terminal_update_generation(ui.get_terminal_update_generation().wrapping_add(1));
    } else if group == 1 {
        ui.set_secondary_terminal_grid_rows(rows.into());
        ui.set_secondary_terminal_grid_columns(columns.into());
        ui.set_secondary_terminal_cursor_row(terminal.cursor_row());
        ui.set_secondary_terminal_cursor_column(terminal.cursor_column());
        update_terminal_model(ui.get_secondary_terminal_cells(), cells, |model| {
            ui.set_secondary_terminal_cells(model)
        });
        ui.set_secondary_terminal_update_generation(
            ui.get_secondary_terminal_update_generation()
                .wrapping_add(1),
        );
    } else {
        edit_extra_pane(ui, group, |pane| {
            pane.terminal_grid_rows = rows.into();
            pane.terminal_grid_columns = columns.into();
            pane.terminal_cursor_row = terminal.cursor_row();
            pane.terminal_cursor_column = terminal.cursor_column();
            update_terminal_model(pane.terminal_cells.clone(), cells, |model| {
                pane.terminal_cells = model;
            });
            pane.terminal_update_generation = pane.terminal_update_generation.wrapping_add(1);
        });
    }
}

fn update_terminal_model(
    current: ModelRc<TerminalCell>,
    cells: Vec<TerminalCell>,
    set_model: impl FnOnce(ModelRc<TerminalCell>),
) {
    let Some(model) = current.as_any().downcast_ref::<VecModel<TerminalCell>>() else {
        set_model(ModelRc::new(VecModel::from(cells)));
        return;
    };

    let old_count = model.row_count();
    if old_count == cells.len()
        && cells.iter().enumerate().all(|(index, cell)| {
            model
                .row_data(index)
                .is_some_and(|current| current.row == cell.row && current.column == cell.column)
        })
    {
        let changed = cells
            .iter()
            .enumerate()
            .filter_map(|(index, cell)| {
                model
                    .row_data(index)
                    .is_none_or(|current| !terminal_cells_equal(&current, cell))
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if changed.len() > 256 && changed.len() * 3 > cells.len() {
            model.set_vec(cells);
        } else {
            for index in changed {
                model.set_row_data(index, cells[index].clone());
            }
        }
        return;
    }

    // Sparse cells are sorted by screen position. Preserve the unchanged
    // prefix and suffix so ordinary typing normally removes the old cursor
    // cell and inserts only the new glyph and cursor cells.
    let prefix = (0..old_count.min(cells.len()))
        .take_while(|index| {
            model
                .row_data(*index)
                .is_some_and(|current| terminal_cells_equal(&current, &cells[*index]))
        })
        .count();
    let max_suffix = (old_count - prefix).min(cells.len() - prefix);
    let suffix = (0..max_suffix)
        .take_while(|offset| {
            let old_index = old_count - 1 - offset;
            let new_index = cells.len() - 1 - offset;
            model
                .row_data(old_index)
                .is_some_and(|current| terminal_cells_equal(&current, &cells[new_index]))
        })
        .count();
    let old_middle_count = old_count - prefix - suffix;
    let new_middle_count = cells.len() - prefix - suffix;

    if old_middle_count + new_middle_count > 256 {
        model.set_vec(cells);
        return;
    }

    for _ in 0..old_middle_count {
        model.remove(prefix);
    }
    for (offset, cell) in cells
        .into_iter()
        .skip(prefix)
        .take(new_middle_count)
        .enumerate()
    {
        model.insert(prefix + offset, cell);
    }
}

fn terminal_cells_equal(left: &TerminalCell, right: &TerminalCell) -> bool {
    left.row == right.row
        && left.column == right.column
        && left.glyph == right.glyph
        && left.foreground == right.foreground
        && left.background == right.background
        && left.bold == right.bold
        && left.cursor == right.cursor
        && left.column_span == right.column_span
}

fn clear_terminal_group(ui: &AppWindow, group: usize) {
    if group == 0 {
        update_terminal_model(ui.get_terminal_cells(), Vec::new(), |model| {
            ui.set_terminal_cells(model)
        });
        ui.set_terminal_cursor_row(-1);
        ui.set_terminal_cursor_column(-1);
    } else if group == 1 {
        update_terminal_model(ui.get_secondary_terminal_cells(), Vec::new(), |model| {
            ui.set_secondary_terminal_cells(model)
        });
        ui.set_secondary_terminal_cursor_row(-1);
        ui.set_secondary_terminal_cursor_column(-1);
    } else {
        edit_extra_pane(ui, group, |pane| {
            update_terminal_model(pane.terminal_cells.clone(), Vec::new(), |model| {
                pane.terminal_cells = model;
            });
            pane.terminal_cursor_row = -1;
            pane.terminal_cursor_column = -1;
        });
    }
}

fn tab_title(tab: &WorkspaceTab) -> String {
    match &tab.content {
        TabContent::File(document) => document.title(),
        TabContent::Diff(diff) => {
            let name = diff
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Diff");
            format!("{name} (Working Tree)")
        }
        TabContent::Terminal {
            start_path, number, ..
        } => {
            let directory = start_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("/");
            format!("Terminal {number} · {directory}")
        }
    }
}

fn tab_detail(tab: &WorkspaceTab) -> String {
    match &tab.content {
        TabContent::File(document) => document.linux_path.to_string_lossy().to_string(),
        TabContent::Diff(diff) => diff.path.to_string_lossy().to_string(),
        TabContent::Terminal { start_path, .. } => start_path.to_string_lossy().to_string(),
    }
}

fn sync_tree(ui: &AppWindow, state: &AppState) {
    let entries = state
        .tree
        .iter()
        .map(|node| {
            let project_kind = if node.is_directory && state.repositories.contains(&node.linux_path)
            {
                "git"
            } else if node.is_directory
                && node.depth == 0
                && !state.repositories.contains(&state.workspace.linux_root)
            {
                "local"
            } else {
                ""
            };
            let icon_label = tree::icon_for(
                &node.name,
                node.is_directory,
                node.is_expanded,
                project_kind,
            );
            TreeEntry {
                name: node.name.clone().into(),
                path: tree::linux_path_text(&node.linux_path).into(),
                icon: state.emoji_icons.get(icon_label),
                icon_label: icon_label.into(),
                depth: node.depth,
                is_directory: node.is_directory,
                is_expanded: node.is_expanded,
                git_mark: match node.git_status {
                    GitStatus::Clean => "",
                    GitStatus::Modified => "M",
                    GitStatus::Untracked => "U",
                    GitStatus::Deleted => "D",
                }
                .into(),
                project_kind: project_kind.into(),
            }
        })
        .collect::<Vec<_>>();
    ui.set_tree_entries(ModelRc::new(VecModel::from(entries)));
}

fn sync_git_changes(ui: &AppWindow, state: &AppState) {
    let mut repositories = state.repositories.iter().cloned().collect::<Vec<_>>();
    repositories.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    let mut entries = Vec::new();
    let mut change_count = 0usize;

    for repository in repositories {
        let mut changes = state
            .statuses
            .iter()
            .filter(|(path, _)| {
                repository_for(&state.repositories, path).as_ref() == Some(&repository)
            })
            .map(|(path, status)| (path.clone(), *status))
            .collect::<Vec<_>>();
        changes.sort_by_key(|(path, _)| path.to_string_lossy().to_ascii_lowercase());
        if changes.is_empty() {
            continue;
        }
        change_count += changes.len();
        let repository_name = repository
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Repository");
        entries.push(GitEntry {
            kind: "repository".into(),
            name: repository_name.into(),
            detail: repository.to_string_lossy().to_string().into(),
            path: repository.to_string_lossy().to_string().into(),
            status: "".into(),
            count: changes.len() as i32,
            icon: slint::Image::default(),
            expanded: state.expanded_git_repositories.contains(&repository),
        });

        if !state.expanded_git_repositories.contains(&repository) {
            continue;
        }
        for (path, status) in changes {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file");
            let relative = path.strip_prefix(&repository).unwrap_or(&path);
            let detail = relative
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.to_string_lossy().to_string())
                .unwrap_or_default();
            let icon_label = tree::icon_for(name, false, false, "");
            entries.push(GitEntry {
                kind: "change".into(),
                name: name.into(),
                detail: detail.into(),
                path: path.to_string_lossy().to_string().into(),
                status: match status {
                    GitStatus::Modified => "M",
                    GitStatus::Untracked => "U",
                    GitStatus::Deleted => "D",
                    GitStatus::Clean => "",
                }
                .into(),
                count: 0,
                icon: state.emoji_icons.get(icon_label),
                expanded: false,
            });
        }
    }
    ui.set_git_change_count(change_count.min(i32::MAX as usize) as i32);
    ui.set_git_entries(ModelRc::new(VecModel::from(entries)));
}

fn sync_tabs(ui: &AppWindow, state: &AppState) {
    let tabs_for_group = |group| {
        state
            .tabs
            .iter()
            .filter(|tab| state.tab_groups.group_of(tab.id) == Some(group))
            .map(|tab| TabEntry {
                id: tab.id as i32,
                title: tab_title(tab).into(),
                detail: tab_detail(tab).into(),
                kind: match tab.content {
                    TabContent::File(_) => "file",
                    TabContent::Diff(_) => "diff",
                    TabContent::Terminal { .. } => "terminal",
                }
                .into(),
                group: group as i32,
                active: state.tab_groups.active(group) == Some(tab.id),
                dirty: matches!(&tab.content, TabContent::File(document) if document.dirty),
            })
            .collect::<Vec<_>>()
    };
    ui.set_primary_tabs(ModelRc::new(VecModel::from(tabs_for_group(0))));
    ui.set_secondary_tabs(ModelRc::new(VecModel::from(tabs_for_group(1))));
    for group in state
        .tab_groups
        .groups()
        .into_iter()
        .filter(|group| *group >= 2)
    {
        edit_extra_pane(ui, group, |pane| {
            pane.tabs = ModelRc::new(VecModel::from(tabs_for_group(group)));
        });
    }
}
