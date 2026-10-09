//! Calculate every terminal's grid from its current pane, including new panes.
// Keep these metrics in sync with WorkspaceGroup's cell and tab-bar dimensions.
pub fn grid_size(width: f32, height: f32, font_size: i32) -> Option<(u16, u16)> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 36.0 || font_size <= 0
    {
        return None;
    }
    let cell_width = (font_size as f32 * 4.0 / 7.0).round().max(1.0);
    let cell_height = (font_size as f32 * 8.0 / 7.0).ceil().max(1.0);
    Some((
        ((height - 36.0) / cell_height)
            .floor()
            .clamp(2.0, u16::MAX as f32) as u16,
        (width / cell_width).floor().clamp(20.0, u16::MAX as f32) as u16,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_and_nested_panes_use_their_actual_size() {
        assert_eq!(grid_size(944.0, 776.0, 14), Some((46, 118)));
        assert_eq!(grid_size(944.0, 388.0, 14), Some((22, 118)));
        assert_eq!(grid_size(472.0, 388.0, 14), Some((22, 59)));
        assert_eq!(grid_size(944.0, 388.0, 24), Some((12, 67)));
    }
    #[test]
    fn transient_empty_geometry_does_not_resize_a_running_tui() {
        for (width, height) in [(0.0, 776.0), (944.0, 0.0), (944.0, 36.0), (f32::NAN, 388.0)] {
            assert_eq!(grid_size(width, height, 14), None);
        }
        assert_eq!(grid_size(944.0, 388.0, 0), None);
        assert_eq!(grid_size(1.0, 37.0, 14), Some((2, 20)));
    }
}
