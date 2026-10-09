//! Focus the relocated terminal now and once after newly created panes settle.
use crate::AppWindow;
use slint::{ComponentHandle, Timer, TimerMode};
use std::time::Duration;

#[derive(Default)]
pub struct TerminalFocusTransfer {
    timer: Timer,
}
impl TerminalFocusTransfer {
    pub fn request(&self, ui: &AppWindow) {
        ui.invoke_focus_terminal();
        ui.window().request_redraw();
        let weak = ui.as_weak();
        // A repeated drop replaces the pending callback. Read the current
        // focused pane when it fires, never an obsolete source/destination ID.
        self.timer
            .start(TimerMode::SingleShot, Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    ui.invoke_focus_terminal();
                }
            });
    }
}
