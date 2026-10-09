#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
use araseo_ui_harness::AppWindow;
#[cfg(windows)]
#[path = "../../src/terminal_focus.rs"]
mod terminal_focus;
#[cfg(windows)]
#[path = "../../src/terminal_layout.rs"]
mod terminal_layout;
#[cfg(windows)]
fn main() -> Result<(), slint::PlatformError> {
    use araseo_harness::codex_fixture;
    use araseo_ui_harness::{PaneEntry, TabEntry, TerminalCell, terminal_view};
    use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
    use std::{
        cell::{Cell, RefCell},
        io::Write,
        rc::Rc,
        time::Duration,
    };
    let args = std::env::args().collect::<Vec<_>>();
    let extra = args.get(1).is_some_and(|a| a == "extra");
    let log_path = args.get(2).expect("log path").clone();
    let log = Rc::new(move |s: &str| {
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&log_path)
                .unwrap(),
            "{s}"
        )
        .unwrap();
    });
    log("REAL_CODEX");
    let (session, dir) = codex_fixture::spawn(if extra {
        "native-extra"
    } else {
        "native-bottom"
    });
    let session = Rc::new(RefCell::new(session));
    let composer_confirmed = Cell::new(false);
    let ui = AppWindow::new()?;
    ui.window().set_size(slint::PhysicalSize::new(1200, 800));
    ui.set_dynamic_panes(true);
    ui.set_primary_active_tab_id(2);
    ui.set_primary_active_kind("terminal".into());
    ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
        id: 2,
        title: "Codex fixture".into(),
        kind: "terminal".into(),
        ..Default::default()
    }])));
    let input_session = session.clone();
    let input_log = log.clone();
    let input_count = Cell::new(0);
    ui.on_terminal_text(move |id, text| {
        let n = input_count.get() + 1;
        input_count.set(n);
        input_log(&format!("TEXT {n} {id}"));
        input_session.borrow_mut().write(text.as_bytes());
    });
    let tick = Timer::default();
    let weak = ui.as_weak();
    let frame_session = session.clone();
    let stage = Cell::new(0usize);
    let tick_log = log.clone();
    let transfer = terminal_focus::TerminalFocusTransfer::default();
    tick.start(TimerMode::Repeated, Duration::from_millis(16), move || {
        let ui = weak.upgrade().unwrap();
        let mut terminal = frame_session.borrow_mut();
        let changed = terminal.poll();
        if !composer_confirmed.get() && codex_fixture::text(&terminal).contains("abcdef") {
            composer_confirmed.set(true);
            tick_log("COMPOSER abcdef");
        }
        if stage.get() == 0 && codex_fixture::text(&terminal).contains("for shortcuts") {
            std::fs::write(dir.join("before.txt"), codex_fixture::text(&terminal)).unwrap();
            ui.set_focused_group(if extra { 2 } else { 1 });
            ui.set_primary_active_tab_id(1);
            ui.set_primary_active_kind("file".into());
            ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
                id: 1,
                title: "file".into(),
                kind: "file".into(),
                ..Default::default()
            }])));
            ui.set_primary_layout_height(0.5);
            let size = terminal_layout::grid_size(
                ui.get_workspace_area_width(),
                ui.get_workspace_area_height() * 0.5,
                14,
            )
            .unwrap();
            terminal.resize(size.0, size.1);
            let tabs = ModelRc::new(VecModel::from(vec![TabEntry {
                id: 2,
                title: "Codex fixture".into(),
                kind: "terminal".into(),
                group: if extra { 2 } else { 1 },
                ..Default::default()
            }]));
            if extra {
                ui.set_extra_panes(ModelRc::new(VecModel::from(vec![PaneEntry {
                    group: 2,
                    x: 0.0,
                    y: 0.5,
                    width: 1.0,
                    height: 0.5,
                    visible: true,
                    active_tab_id: 2,
                    active_kind: "terminal".into(),
                    tabs,
                    ..Default::default()
                }])));
            } else {
                ui.set_secondary_layout_y(0.5);
                ui.set_secondary_layout_width(1.0);
                ui.set_secondary_layout_height(0.5);
                ui.set_secondary_layout_visible(true);
                ui.set_secondary_active_tab_id(2);
                ui.set_secondary_active_kind("terminal".into());
                ui.set_secondary_tabs(tabs);
            }
            stage.set(if extra { 2 } else { 1 });
            transfer.request(&ui);
            tick_log("READY");
        }
        let ratio = if stage.get() == 0 { 1.0 } else { 0.5 };
        if let Some(size) = terminal_layout::grid_size(
            ui.get_workspace_area_width(),
            ui.get_workspace_area_height() * ratio,
            14,
        ) {
            terminal.resize(size.0, size.1);
        }
        if changed {
            let size = terminal.size();
            let cells = terminal
                .cells()
                .into_iter()
                .map(|c| TerminalCell {
                    row: c.row,
                    column: c.column,
                    glyph: c.glyph.into(),
                    foreground: slint::Color::from_rgb_u8(
                        c.foreground[0],
                        c.foreground[1],
                        c.foreground[2],
                    ),
                    background: slint::Color::from_rgb_u8(
                        c.background[0],
                        c.background[1],
                        c.background[2],
                    ),
                    bold: c.bold,
                    cursor: c.cursor,
                    column_span: c.column_span,
                })
                .collect();
            terminal_view::sync_frame(
                &ui,
                stage.get(),
                size.0,
                size.1,
                terminal.cursor_row(),
                terminal.cursor_column(),
                cells,
            );
            if stage.get() != 0 {
                std::fs::write(dir.join("after.txt"), codex_fixture::text(&terminal)).unwrap();
            }
        }
    });
    ui.run()
}
#[cfg(not(windows))]
fn main() {}
