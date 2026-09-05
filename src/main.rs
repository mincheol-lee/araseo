#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod document;
mod emoji;
mod git;
mod highlight;
mod tabs;
mod terminal;
mod tree;
mod workspace;

use anyhow::{Context, Result, bail};
use document::{Document, line_numbers};
use slint::{ModelRc, Timer, TimerMode, VecModel};
use slint::winit_030::WinitWindowAccessor;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use tabs::{TabGroups, TabId};
use terminal::TerminalSession;
use tree::{FlatNode, GitStatus};
use workspace::Workspace;

slint::include_modules!();

struct AppState {
    workspace: Workspace,
    expanded: HashSet<PathBuf>,
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
}

enum TabContent {
    File(Document),
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
    let workspace = Workspace::new(distro, root)?;
    let statuses = HashMap::new();
    let tree = tree::build_tree(&workspace, &HashSet::new(), &statuses)?;
    let (git_monitor, monitor_error) = match git::StatusMonitor::spawn(&workspace) {
        Ok(monitor) => (Some(monitor), None),
        Err(error) => (None, Some(format!("Git auto-refresh unavailable: {error}"))),
    };
    let (terminal, initial_status) =
        match TerminalSession::spawn(&workspace.distro, &workspace.linux_root) {
            Ok(terminal) => (Some(terminal), monitor_error.unwrap_or_else(|| "Ready".to_string())),
            Err(error) => (None, format!("Terminal unavailable: {error}")),
        };
    let mut workspace_tabs = Vec::new();
    let mut tab_groups = TabGroups::default();
    let mut next_tab_id = 0;
    let mut next_terminal_number = 1;
    if let Some(terminal) = terminal {
        workspace_tabs.push(WorkspaceTab {
            id: next_tab_id,
            content: TabContent::Terminal {
                session: terminal,
                start_path: workspace.linux_root.clone(),
                number: next_terminal_number,
            },
        });
        tab_groups.add(next_tab_id, 0);
        next_tab_id += 1;
        next_terminal_number += 1;
    }
    let state = Rc::new(RefCell::new(AppState {
        workspace,
        expanded: HashSet::new(),
        statuses,
        repositories: HashSet::new(),
        tree,
        tabs: workspace_tabs,
        tab_groups,
        next_tab_id,
        next_terminal_number,
        git_monitor,
        status: initial_status,
        save_conflict: None,
        syncing_editor: Cell::new(false),
        emoji_icons: emoji::EmojiIcons::load_system(),
    }));

    let ui = AppWindow::new()?;
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
            if is_active
                && let Some(document) = document_mut(&mut state, tab_id)
            {
                document.set_text(text.to_string());
            }
            if let Some(ui) = weak.upgrade() {
                sync_tabs(&ui, &state);
                if let Some(group) = state.tab_groups.group_of(tab_id) {
                    sync_group(&ui, &state, group);
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
        let state = state.clone();
        ui.on_terminal_key(move |tab_id, text, control, alt, shift| {
            let Ok(tab_id) = TabId::try_from(tab_id) else {
                return;
            };
            if let Some(terminal) = terminal_ref(&state.borrow(), tab_id) {
                terminal.write(&terminal::encode_key(&text, control, alt, shift));
            }
        });
    }
    {
        let state = state.clone();
        ui.on_terminal_text(move |tab_id, text| {
            let Ok(tab_id) = TabId::try_from(tab_id) else {
                return;
            };
            if let Some(terminal) = terminal_ref(&state.borrow(), tab_id) {
                terminal.write(text.as_bytes());
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
            let group = usize::try_from(group).unwrap_or(0).min(1);
            if let Err(error) = open_terminal(&mut state, group) {
                state.status = format!("Terminal unavailable: {error}");
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
                ui.invoke_focus_terminal();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tab_dock_requested(move |tab_id, zone| {
            let mut state = state.borrow_mut();
            if let Ok(tab_id) = TabId::try_from(tab_id) {
                state.tab_groups.dock(tab_id, zone);
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
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
    ui.on_terminal_paste(|_| {});

    let timer = Timer::default();
    {
        let weak = ui.as_weak();
        let state = state.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
            let Some(ui) = weak.upgrade() else { return };
            let terminal_sizes = [
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
            let active = [state.tab_groups.active(0), state.tab_groups.active(1)];
            let mut changed_groups = [false, false];
            for tab in &mut state.tabs {
                if let TabContent::Terminal { session, .. } = &mut tab.content {
                    let visible_group = if active[0] == Some(tab.id) {
                        Some(0)
                    } else if active[1] == Some(tab.id) {
                        Some(1)
                    } else {
                        None
                    };
                    let resized = visible_group.is_some_and(|group| {
                        let (rows, columns) = terminal_sizes[group];
                        session.resize(rows, columns)
                    });
                    let changed = resized | session.poll();
                    if changed
                        && let Some(group) = visible_group
                    {
                        changed_groups[group] = true;
                    }
                }
            }
            for (group, changed) in changed_groups.into_iter().enumerate() {
                if changed {
                    sync_group(&ui, &state, group);
                }
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
            let previous_repositories = state.repositories.clone();
            let previous_status = state.status.clone();
            state.statuses = snapshot.statuses;
            state.repositories = snapshot.repositories;
            if state.status == "Refreshing..." {
                state.status = "Refreshed".into();
            }
            refresh_tree(&mut state);
            if state.tree != previous_tree || state.repositories != previous_repositories {
                sync_tree(&ui, &state);
            }
            if state.status != previous_status {
                ui.set_status_text(state.status.clone().into());
            }
        });
    }

    ui.run()?;
    drop(timer);
    drop(workspace_timer);
    Ok(())
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
            } else {
                ui.set_secondary_editor_cursor_offset(cursor.min(i32::MAX as usize) as i32);
                ui.set_secondary_editor_cursor_generation(
                    ui.get_secondary_editor_cursor_generation().wrapping_add(1),
                );
            }
            state.syncing_editor.set(false);
            sync_tabs(&ui, &state);
        }
    }
}

fn open_document(state: &mut AppState, linux_path: PathBuf) -> Result<()> {
    if let Some(tab_id) = state.tabs.iter().find_map(|tab| match &tab.content {
        TabContent::File(document) if document.linux_path == linux_path => Some(tab.id),
        _ => None,
    }) {
        state.tab_groups.activate(tab_id);
        return Ok(());
    }

    let host_path = state.workspace.host_path(&linux_path)?;
    let document = Document::open(linux_path, host_path)?;
    let tab_id = take_next_tab_id(state);
    let group = state.tab_groups.focused_group();
    state.tabs.push(WorkspaceTab {
        id: tab_id,
        content: TabContent::File(document),
    });
    state.tab_groups.add(tab_id, group);
    state.status = "File opened".into();
    Ok(())
}

fn open_terminal(state: &mut AppState, group: usize) -> Result<TabId> {
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

fn take_next_tab_id(state: &mut AppState) -> TabId {
    let id = state.next_tab_id;
    state.next_tab_id = state.next_tab_id.wrapping_add(1);
    id
}

fn focused_tab_id(state: &AppState) -> Option<TabId> {
    state.tab_groups.active(state.tab_groups.focused_group())
}

fn document_ref(state: &AppState, tab_id: TabId) -> Option<&Document> {
    state.tabs.iter().find_map(|tab| {
        if tab.id != tab_id {
            return None;
        }
        match &tab.content {
            TabContent::File(document) => Some(document),
            TabContent::Terminal { .. } => None,
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
            TabContent::Terminal { .. } => None,
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
            TabContent::File(_) => None,
        }
    })
}

fn refresh_tree(state: &mut AppState) {
    match tree::build_tree(&state.workspace, &state.expanded, &state.statuses) {
        Ok(tree) => state.tree = tree,
        Err(error) => state.status = error.to_string(),
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
    ui.set_focused_group(state.tab_groups.focused_group() as i32);
    sync_tree(ui, state);
    sync_tabs(ui, state);
    sync_group(ui, state, 0);
    sync_group(ui, state, 1);

    let active_path = focused_tab_id(state)
        .and_then(|tab_id| state.tabs.iter().find(|tab| tab.id == tab_id))
        .map(tab_detail)
        .unwrap_or_default();
    ui.set_active_path(active_path.into());
}

fn sync_group(ui: &AppWindow, state: &AppState, group: usize) {
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
                    ui.set_editor_text(document.text.clone().into());
                    state.syncing_editor.set(false);
                    ui.set_line_numbers(line_numbers(&document.text).into());
                    sync_highlight_for_document(ui, document, 0);
                    clear_terminal_group(ui, 0);
                }
                TabContent::Terminal { session, .. } => {
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
                    ui.set_secondary_editor_text(document.text.clone().into());
                    state.syncing_editor.set(false);
                    ui.set_secondary_line_numbers(line_numbers(&document.text).into());
                    sync_highlight_for_document(ui, document, 1);
                    clear_terminal_group(ui, 1);
                }
                TabContent::Terminal { session, .. } => {
                    ui.set_secondary_active_kind("terminal".into());
                    ui.set_secondary_syntax_highlight_enabled(false);
                    ui.set_secondary_highlighted_text(slint::StyledText::default());
                    sync_terminal(ui, session, 1);
                }
            }
        }
        (0, None) => {
            ui.set_primary_active_tab_id(-1);
            ui.set_primary_active_kind("".into());
            ui.set_primary_active_title("".into());
            ui.set_primary_active_detail("".into());
            ui.set_editor_text("".into());
            ui.set_line_numbers("1".into());
            ui.set_syntax_highlight_enabled(false);
            ui.set_highlighted_text(slint::StyledText::default());
            clear_terminal_group(ui, 0);
        }
        (1, None) => {
            ui.set_secondary_active_tab_id(-1);
            ui.set_secondary_active_kind("".into());
            ui.set_secondary_active_title("".into());
            ui.set_secondary_active_detail("".into());
            ui.set_secondary_editor_text("".into());
            ui.set_secondary_line_numbers("1".into());
            ui.set_secondary_syntax_highlight_enabled(false);
            ui.set_secondary_highlighted_text(slint::StyledText::default());
            clear_terminal_group(ui, 1);
        }
        _ => {}
    }
}

fn sync_highlight_for_document(ui: &AppWindow, document: &Document, group: usize) {
    let highlighted = highlight::highlighted(&document.linux_path, &document.text);
    match (group, highlighted) {
        (0, Some(text)) => {
            ui.set_highlighted_text(text);
            ui.set_syntax_highlight_enabled(true);
        }
        (1, Some(text)) => {
            ui.set_secondary_highlighted_text(text);
            ui.set_secondary_syntax_highlight_enabled(true);
        }
        (0, None) => {
            ui.set_highlighted_text(slint::StyledText::default());
            ui.set_syntax_highlight_enabled(false);
        }
        (1, None) => {
            ui.set_secondary_highlighted_text(slint::StyledText::default());
            ui.set_secondary_syntax_highlight_enabled(false);
        }
        _ => {}
    }
}

fn sync_terminal(ui: &AppWindow, terminal: &TerminalSession, group: usize) {
    let (rows, columns) = terminal.size();
    let cells = terminal
        .cells()
        .into_iter()
        .map(|cell| TerminalCell {
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
    let cells = ModelRc::new(VecModel::from(cells));
    if group == 0 {
        ui.set_terminal_grid_rows(rows.into());
        ui.set_terminal_grid_columns(columns.into());
        ui.set_terminal_cursor_row(terminal.cursor_row());
        ui.set_terminal_cursor_column(terminal.cursor_column());
        ui.set_terminal_cells(cells);
        ui.set_terminal_update_generation(ui.get_terminal_update_generation().wrapping_add(1));
    } else {
        ui.set_secondary_terminal_grid_rows(rows.into());
        ui.set_secondary_terminal_grid_columns(columns.into());
        ui.set_secondary_terminal_cursor_row(terminal.cursor_row());
        ui.set_secondary_terminal_cursor_column(terminal.cursor_column());
        ui.set_secondary_terminal_cells(cells);
        ui.set_secondary_terminal_update_generation(
            ui.get_secondary_terminal_update_generation().wrapping_add(1),
        );
    }
}

fn clear_terminal_group(ui: &AppWindow, group: usize) {
    let cells = ModelRc::new(VecModel::<TerminalCell>::default());
    if group == 0 {
        ui.set_terminal_cells(cells);
        ui.set_terminal_cursor_row(-1);
        ui.set_terminal_cursor_column(-1);
    } else {
        ui.set_secondary_terminal_cells(cells);
        ui.set_secondary_terminal_cursor_row(-1);
        ui.set_secondary_terminal_cursor_column(-1);
    }
}

fn tab_title(tab: &WorkspaceTab) -> String {
    match &tab.content {
        TabContent::File(document) => document.title(),
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
        TabContent::Terminal { start_path, .. } => start_path.to_string_lossy().to_string(),
    }
}

fn sync_tree(ui: &AppWindow, state: &AppState) {
    let entries = state
        .tree
        .iter()
        .map(|node| {
            let project_kind = if node.is_directory
                && state.repositories.contains(&node.linux_path)
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
                icon: state.emoji_icons.get(icon_label),
                icon_label: icon_label.into(),
                depth: node.depth,
                is_directory: node.is_directory,
                is_expanded: node.is_expanded,
                git_mark: match node.git_status {
                    GitStatus::Clean => "",
                    GitStatus::Modified => "M",
                    GitStatus::Untracked => "U",
                }
                .into(),
                project_kind: project_kind.into(),
            }
        })
        .collect::<Vec<_>>();
    ui.set_tree_entries(ModelRc::new(VecModel::from(entries)));
}

fn sync_tabs(ui: &AppWindow, state: &AppState) {
    let focused_id = focused_tab_id(state);
    let active_index = focused_id
        .and_then(|id| state.tabs.iter().position(|tab| tab.id == id))
        .map(|index| index as i32)
        .unwrap_or(-1);
    let tabs = state
        .tabs
        .iter()
        .map(|tab| {
            let group = state.tab_groups.group_of(tab.id).unwrap_or(0);
            TabEntry {
                id: tab.id as i32,
                title: tab_title(tab).into(),
                detail: tab_detail(tab).into(),
                kind: match tab.content {
                    TabContent::File(_) => "file",
                    TabContent::Terminal { .. } => "terminal",
                }
                .into(),
                group: group as i32,
                active: state.tab_groups.active(group) == Some(tab.id),
                dirty: matches!(&tab.content, TabContent::File(document) if document.dirty),
            }
        })
        .collect::<Vec<_>>();
    ui.set_active_tab(active_index);
    ui.set_tabs(ModelRc::new(VecModel::from(tabs)));
}
