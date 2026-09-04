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
    documents: Vec<Document>,
    active_document: Option<usize>,
    terminal: Option<TerminalSession>,
    git_monitor: Option<git::StatusMonitor>,
    status: String,
    save_conflict: bool,
    syncing_editor: Cell<bool>,
    emoji_icons: emoji::EmojiIcons,
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
    let state = Rc::new(RefCell::new(AppState {
        workspace,
        expanded: HashSet::new(),
        statuses,
        repositories: HashSet::new(),
        tree,
        documents: Vec::new(),
        active_document: None,
        terminal,
        git_monitor,
        status: initial_status,
        save_conflict: false,
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
        ui.on_undo_requested(move |document_index| {
            apply_history_change(&weak, &state, document_index, true);
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_redo_requested(move |document_index| {
            apply_history_change(&weak, &state, document_index, false);
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tab_cycle(move |delta| {
            let mut state = state.borrow_mut();
            state.active_document =
                tabs::cycle_index(state.active_document, state.documents.len(), delta);
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tab_activated(move |index| {
            let mut state = state.borrow_mut();
            if (index as usize) < state.documents.len() {
                state.active_document = Some(index as usize);
            }
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_tab_close(move |index| {
            let mut state = state.borrow_mut();
            let index = index as usize;
            if let Some(document) = state.documents.get(index) {
                if document.dirty {
                    state.status = "Save the modified file before closing it".into();
                } else {
                    state.documents.remove(index);
                    state.active_document = if state.documents.is_empty() {
                        None
                    } else {
                        Some(index.min(state.documents.len() - 1))
                    };
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
        ui.on_editor_edited(move |text, document_index| {
            if state.borrow().syncing_editor.get() {
                return;
            }
            let mut state = state.borrow_mut();
            let document_index = usize::try_from(document_index).ok();
            if let Some(index) = document_index
                && state.active_document == Some(index)
                && index < state.documents.len()
            {
                state.documents[index].set_text(text.to_string());
            }
            if let Some(ui) = weak.upgrade() {
                sync_tabs(&ui, &state);
                sync_highlight(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_save_requested(move || {
            let mut state = state.borrow_mut();
            save_active(&mut state, false);
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
            save_active(&mut state, true);
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
            reload_active(&mut state);
            if let Some(ui) = weak.upgrade() {
                sync_ui(&ui, &state);
            }
        });
    }
    {
        let state = state.clone();
        ui.on_terminal_key(move |text, control, alt, shift| {
            if let Some(terminal) = &state.borrow().terminal {
                terminal.write(&terminal::encode_key(&text, control, alt, shift));
            }
        });
    }
    {
        let state = state.clone();
        ui.on_terminal_text(move |text| {
            if let Some(terminal) = &state.borrow().terminal {
                terminal.write(text.as_bytes());
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_find_requested(move |query| {
            let mut state = state.borrow_mut();
            state.status = match state
                .active_document
                .and_then(|index| state.documents[index].text.find(query.as_str()))
            {
                Some(offset) if !query.is_empty() => format!("Found at byte {}", offset + 1),
                _ if query.is_empty() => "Find".into(),
                _ => "No match".into(),
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_status_text(state.status.clone().into());
            }
        });
    }
    ui.on_terminal_paste(|_| {});

    let timer = Timer::default();
    {
        let weak = ui.as_weak();
        let state = state.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
            let Some(ui) = weak.upgrade() else { return };
            let rows = ui.get_terminal_rows().round().clamp(2.0, u16::MAX as f32) as u16;
            let columns = ui
                .get_terminal_columns()
                .round()
                .clamp(20.0, u16::MAX as f32) as u16;
            let mut state = state.borrow_mut();
            let changed = state
                .terminal
                .as_mut()
                .is_some_and(|terminal| terminal.resize(rows, columns) | terminal.poll());
            if changed {
                if let Some(terminal) = state.terminal.as_ref() {
                    sync_terminal(&ui, terminal);
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
    document_index: i32,
    undo: bool,
) {
    let Some(index) = usize::try_from(document_index).ok() else {
        return;
    };
    let mut state = state.borrow_mut();
    if state.active_document != Some(index) || index >= state.documents.len() {
        return;
    }
    let cursor = if undo {
        state.documents[index].undo()
    } else {
        state.documents[index].redo()
    };
    let Some(cursor) = cursor else {
        return;
    };
    if let Some(ui) = weak.upgrade() {
        state.syncing_editor.set(true);
        ui.set_editor_text(state.documents[index].text.clone().into());
        ui.set_editor_cursor_offset(cursor.min(i32::MAX as usize) as i32);
        ui.set_editor_cursor_generation(ui.get_editor_cursor_generation().wrapping_add(1));
        state.syncing_editor.set(false);
        ui.set_line_numbers(line_numbers(&state.documents[index].text).into());
        sync_tabs(&ui, &state);
        sync_highlight(&ui, &state);
    }
}

fn open_document(state: &mut AppState, linux_path: PathBuf) -> Result<()> {
    if let Some(index) = state
        .documents
        .iter()
        .position(|document| document.linux_path == linux_path)
    {
        state.active_document = Some(index);
        return Ok(());
    }
    let host_path = state.workspace.host_path(&linux_path)?;
    let document = Document::open(linux_path, host_path)?;
    state.documents.push(document);
    state.active_document = Some(state.documents.len() - 1);
    state.status = "File opened".into();
    Ok(())
}

fn save_active(state: &mut AppState, overwrite_external: bool) {
    let Some(index) = state.active_document else {
        return;
    };
    match state.documents[index].save(overwrite_external) {
        Ok(()) => {
            state.status = "Saved".into();
            state.save_conflict = false;
            if let Some(monitor) = state.git_monitor.as_mut() {
                let _ = monitor.force_refresh();
            } else {
                state.statuses = git::read_status(&state.workspace);
            }
            refresh_tree(state);
        }
        Err(error) => {
            state.save_conflict = state.documents[index].changed_on_disk();
            state.status = error.to_string();
        }
    }
}

fn reload_active(state: &mut AppState) {
    let Some(index) = state.active_document else {
        return;
    };
    let linux_path = state.documents[index].linux_path.clone();
    let host_path = state.documents[index].host_path.clone();
    match Document::open(linux_path, host_path) {
        Ok(document) => {
            state.documents[index] = document;
            state.save_conflict = false;
            state.status = "Reloaded from disk".into();
        }
        Err(error) => state.status = error.to_string(),
    }
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
    ui.set_save_conflict(state.save_conflict);
    sync_tree(ui, state);
    sync_tabs(ui, state);
    if let Some(index) = state.active_document {
        let document = &state.documents[index];
        ui.set_active_tab(index as i32);
        state.syncing_editor.set(true);
        ui.set_editor_text(document.text.clone().into());
        state.syncing_editor.set(false);
        ui.set_line_numbers(line_numbers(&document.text).into());
        ui.set_active_path(document.linux_path.to_string_lossy().to_string().into());
        sync_highlight(ui, state);
    } else {
        ui.set_active_tab(-1);
        ui.set_editor_text("".into());
        ui.set_line_numbers("1".into());
        ui.set_active_path("".into());
        ui.set_syntax_highlight_enabled(false);
        ui.set_highlighted_text(slint::StyledText::default());
    }
    if let Some(terminal) = &state.terminal {
        sync_terminal(ui, terminal);
    }
}

fn sync_highlight(ui: &AppWindow, state: &AppState) {
    let highlighted = state.active_document.and_then(|index| {
        let document = &state.documents[index];
        highlight::highlighted(&document.linux_path, &document.text)
    });
    match highlighted {
        Some(text) => {
            ui.set_highlighted_text(text);
            ui.set_syntax_highlight_enabled(true);
        }
        None => {
            ui.set_highlighted_text(slint::StyledText::default());
            ui.set_syntax_highlight_enabled(false);
        }
    }
}

fn sync_terminal(ui: &AppWindow, terminal: &TerminalSession) {
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
    ui.set_terminal_grid_rows(rows.into());
    ui.set_terminal_grid_columns(columns.into());
    ui.set_terminal_cursor_row(terminal.cursor_row());
    ui.set_terminal_cursor_column(terminal.cursor_column());
    ui.set_terminal_cells(ModelRc::new(VecModel::from(cells)));
    ui.set_terminal_update_generation(ui.get_terminal_update_generation().wrapping_add(1));
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
    let tabs = state
        .documents
        .iter()
        .map(|document| TabEntry {
            title: document.title().into(),
            dirty: document.dirty,
        })
        .collect::<Vec<_>>();
    ui.set_tabs(ModelRc::new(VecModel::from(tabs)));
}
