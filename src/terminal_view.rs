//! Production terminal UI updates, shared with native integration fixtures.
use crate::{AppWindow, PaneEntry, TerminalCell};
use slint::{Model, ModelRc, VecModel};

pub fn edit_extra_pane(ui: &AppWindow, group: usize, edit: impl FnOnce(&mut PaneEntry)) {
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

pub fn extra_pane(ui: &AppWindow, group: usize) -> Option<PaneEntry> {
    ui.get_extra_panes().row_data(group.checked_sub(2)?)
}

pub fn sync_frame(
    ui: &AppWindow,
    group: usize,
    rows: u16,
    columns: u16,
    cursor_row: i32,
    cursor_column: i32,
    cells: Vec<TerminalCell>,
) {
    if group == 0 {
        ui.set_terminal_grid_rows(rows.into());
        ui.set_terminal_grid_columns(columns.into());
        ui.set_terminal_cursor_row(cursor_row);
        ui.set_terminal_cursor_column(cursor_column);
        update_terminal_model(ui.get_terminal_cells(), cells, |model| {
            ui.set_terminal_cells(model)
        });
        ui.set_terminal_update_generation(ui.get_terminal_update_generation().wrapping_add(1));
    } else if group == 1 {
        ui.set_secondary_terminal_grid_rows(rows.into());
        ui.set_secondary_terminal_grid_columns(columns.into());
        ui.set_secondary_terminal_cursor_row(cursor_row);
        ui.set_secondary_terminal_cursor_column(cursor_column);
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
            pane.terminal_cursor_row = cursor_row;
            pane.terminal_cursor_column = cursor_column;
            update_terminal_model(pane.terminal_cells.clone(), cells, |model| {
                pane.terminal_cells = model;
            });
            pane.terminal_update_generation = pane.terminal_update_generation.wrapping_add(1);
        });
    }
}

pub fn update_terminal_model(
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
