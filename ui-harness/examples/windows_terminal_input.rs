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
    use araseo_ui_harness::{PaneEntry, TabEntry, TerminalCell};
    use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
    use std::{cell::Cell, io::Write, rc::Rc, time::Duration};
    let args = std::env::args().collect::<Vec<_>>();
    let extra = args.get(1).is_some_and(|s| s == "extra");
    let path = args.get(2).expect("log path").clone();
    let log = Rc::new(move |line: &str| {
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap(),
            "{line}"
        )
        .unwrap();
    });
    let ui = AppWindow::new()?;
    ui.window().set_size(slint::PhysicalSize::new(1200, 800));
    ui.set_dynamic_panes(true);
    ui.set_primary_active_tab_id(2);
    ui.set_primary_active_kind("terminal".into());
    ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
        id: 2,
        title: "Input fixture".into(),
        kind: "terminal".into(),
        ..Default::default()
    }])));
    let transfer = terminal_focus::TerminalFocusTransfer::default();
    let count = Cell::new(0);
    let weak = ui.as_weak();
    let input_log = log.clone();
    ui.on_terminal_text(move |id, _| {
        let n = count.get() + 1;
        count.set(n);
        input_log(&format!("TEXT {n} {id}"));
        if let Some(ui) = weak.upgrade() {
            let row = if extra {
                ui.get_extra_panes().row_data(0).unwrap().terminal_grid_rows - 1
            } else {
                ui.get_secondary_terminal_grid_rows() - 1
            };
            let cells = ModelRc::new(VecModel::from(vec![TerminalCell {
                row,
                column: n,
                glyph: "X".into(),
                column_span: 1,
                foreground: slint::Color::from_rgb_u8(255, 255, 255),
                background: slint::Color::from_rgb_u8(0x28, 0x2c, 0x34),
                ..Default::default()
            }]));
            if extra {
                let panes = ui.get_extra_panes();
                let model = panes
                    .as_any()
                    .downcast_ref::<VecModel<PaneEntry>>()
                    .unwrap();
                let mut pane = model.row_data(0).unwrap();
                pane.preview_blocks = ModelRc::new(VecModel::from(Vec::new()));
                pane.pdf_selection = ModelRc::new(VecModel::from(Vec::new()));
                pane.terminal_cursor_row = row;
                pane.terminal_cursor_column = n;
                pane.terminal_cells = cells;
                pane.terminal_update_generation += 1;
                model.set_row_data(0, pane);
            } else {
                ui.set_secondary_preview_blocks(ModelRc::new(VecModel::from(Vec::new())));
                ui.set_secondary_active_kind("terminal".into());
                ui.set_secondary_terminal_cells(cells);
                ui.set_secondary_terminal_cursor_row(row);
                ui.set_secondary_terminal_cursor_column(n);
                ui.set_secondary_terminal_update_generation(
                    ui.get_secondary_terminal_update_generation() + 1,
                );
            }
            ui.set_status_text(format!("Input {n}").into());
        }
    });
    let move_timer = Timer::default();
    let weak = ui.as_weak();
    move_timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(700),
        move || {
            let ui = weak.upgrade().unwrap();
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
            let tabs = ModelRc::new(VecModel::from(vec![TabEntry {
                id: 2,
                title: "Input fixture".into(),
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
                    terminal_grid_rows: size.0.into(),
                    terminal_grid_columns: size.1.into(),
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
                ui.set_secondary_terminal_grid_rows(size.0.into());
                ui.set_secondary_terminal_grid_columns(size.1.into());
            }
            transfer.request(&ui);
            log("READY");
        },
    );
    ui.run()
}
#[cfg(not(windows))]
fn main() {}
