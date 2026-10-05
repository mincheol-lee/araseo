//! One coalesced final repaint after native window sizing settles.
use slint::{Timer, TimerMode};
use std::time::Duration;

#[derive(Default)]
pub struct ResizeRefresh {
    timer: Timer,
}

impl ResizeRefresh {
    pub fn request(&self, redraw: impl Fn() + 'static) {
        redraw();
        // Restarting the same timer discards intermediate requests. Windows
        // may preserve/move client pixels after the first resize frame.
        self.timer
            .start(TimerMode::SingleShot, Duration::from_millis(50), redraw);
    }

    pub fn cancel(&self) {
        self.timer.stop();
    }
}
