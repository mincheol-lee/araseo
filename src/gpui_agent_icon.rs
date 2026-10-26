//! A static terminal-tab logo selected from the detected agent process.
use crate::terminal::AgentKind;
use gpui_kit::component::Icon;
use gpui_kit::{base::TestSupportExt, prelude::*, *};

pub struct AgentTabIcon {
    kind: Option<AgentKind>,
    logo: Icon,
}
impl AgentTabIcon {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            kind: None,
            logo: Icon::empty(),
        }
    }
    pub fn set_kind(&mut self, kind: Option<AgentKind>, cx: &mut Context<Self>) {
        if self.kind == kind {
            return;
        }
        self.logo = match kind {
            Some(AgentKind::Claude) => Icon::default()
                .data(include_bytes!("../assets/claude.svg"))
                .text_color(rgb(0xd97757)),
            Some(AgentKind::Codex) => Icon::default()
                .data(include_bytes!("../assets/codex.svg"))
                .text_color(rgb(0xf2f3f5)),
            None => Icon::empty(),
        }
        .size(px(16.));
        self.kind = kind;
        cx.notify();
    }
}
impl Render for AgentTabIcon {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let Some(kind) = self.kind else {
            return div().into_any_element();
        };
        let name = match kind {
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "Codex",
        };
        div()
            .id("agent-tab-icon")
            .test_support()
            .role(accesskit::Role::Image)
            .aria_label(name)
            .size(px(16.))
            .flex_shrink_0()
            .child(self.logo.clone())
            .into_any_element()
    }
}
