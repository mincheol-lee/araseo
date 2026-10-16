//! A retained tab icon with bounded animation and no terminal content updates.
use crate::terminal::{AgentKind, AgentStatus};
use gpui_kit::component::Icon;
use gpui_kit::{base::TestSupportExt, prelude::*, *};
use std::time::Duration;

pub struct AgentTabIcon {
    status: Option<AgentStatus>,
    logo: Icon,
    activation: Option<Subscription>,
}

impl AgentTabIcon {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            status: None,
            logo: Icon::empty(),
            activation: None,
        }
    }

    pub fn set_status(&mut self, status: Option<AgentStatus>, cx: &mut Context<Self>) {
        if self.status == status {
            return;
        }
        if self.status.map(|s| s.kind) != status.map(|s| s.kind) {
            // Retain the SVG data across animation frames; GPUI caches its
            // rasterized mask and rotates the existing image on the GPU.
            self.logo = match status.map(|s| s.kind) {
                Some(AgentKind::Claude) => Icon::default()
                    .data(include_bytes!("../assets/claude.svg"))
                    .text_color(rgb(0xd97757)),
                Some(AgentKind::Codex) => Icon::default()
                    .data(include_bytes!("../assets/codex.svg"))
                    .text_color(rgb(0xf2f3f5)),
                None => Icon::empty(),
            }
            .size(px(16.));
        }
        self.status = status;
        cx.notify();
    }
}

impl Render for AgentTabIcon {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.activation.is_none() {
            self.activation = Some(cx.observe_window_activation(window, |_, _, cx| cx.notify()));
        }
        let Some(status) = self.status else {
            return div().into_any_element();
        };
        let name = match status.kind {
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "Codex",
        };
        let logo = if status.running && window.is_window_active() {
            self.logo
                .clone()
                .with_animation(
                    "agent-rotation",
                    Animation::new(Duration::from_millis(800))
                        .repeat_synced()
                        .with_max_fps(10.),
                    |icon, phase| icon.rotate(radians(phase * std::f32::consts::TAU)),
                )
                .into_any_element()
        } else {
            self.logo.clone().into_any_element()
        };
        div()
            .id("agent-tab-icon")
            .test_support()
            .role(accesskit::Role::Image)
            .aria_label(format!(
                "{name} {}",
                if status.running { "running" } else { "waiting" }
            ))
            .size(px(16.))
            .flex_shrink_0()
            .child(logo)
            .into_any_element()
    }
}
