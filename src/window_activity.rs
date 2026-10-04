use std::cell::Cell;

/// Gates work that is only useful while the window is in the foreground.
pub struct WindowActivity {
    focused: Cell<bool>,
}

impl Default for WindowActivity {
    fn default() -> Self {
        Self {
            focused: Cell::new(true),
        }
    }
}

impl WindowActivity {
    pub fn set_focused(&self, focused: bool) {
        self.focused.set(focused);
    }

    pub fn should_probe_agents(&self) -> bool {
        self.focused.get()
    }

    pub fn should_animate_agents(&self, any_running: bool) -> bool {
        self.focused.get() && any_running
    }
}

#[cfg(test)]
mod tests {
    use super::WindowActivity;

    #[test]
    fn foreground_work_pauses_on_blur_and_resumes_on_focus() {
        let activity = WindowActivity::default();
        assert!(activity.should_probe_agents());
        assert!(activity.should_animate_agents(true));
        assert!(!activity.should_animate_agents(false));

        activity.set_focused(false);
        assert!(!activity.should_probe_agents());
        assert!(!activity.should_animate_agents(true));

        activity.set_focused(true);
        assert!(activity.should_probe_agents());
        assert!(activity.should_animate_agents(true));
    }
}
