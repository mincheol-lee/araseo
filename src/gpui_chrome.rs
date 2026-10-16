//! Put window chrome beside the upper-right dock tabs, without a second header.
//! The Kit still owns tab dragging, overflow, splitting, focus and persistence.
use gpui_kit::component::{
    Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    dock::{
        BasePanelView, DockArea, DockAreaRenderer, DockContext, DockPlacement, DockSkin,
        DropIndicator, NodeId, PaneNode, PaneRef, PanelState, TabGroupContext, TabGroupRenderer,
    },
};
use gpui_kit::{base::TestSupportExt, prelude::*, *};
use std::{borrow::Cow, cell::Cell, rc::Rc, sync::Arc};

pub const HEADER_HEIGHT: f32 = 34.;

gpui_kit::assets::icon_assets!(HeaderIcons, [FilePlus, FolderPlus, PanelsTopLeft]);
pub struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == "araseo-app-icon.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/araseo-app-icon.png"
            ))));
        }
        if let Some(bytes) = HeaderIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(HeaderIcons.list(path)?);
        if "araseo-app-icon.png".starts_with(path) {
            paths.push("araseo-app-icon.png".into());
        }
        Ok(paths)
    }
}

type CaptionAction = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct CompactDock {
    skin: Rc<DockSkin>,
    caption: Rc<Cell<Option<NodeId>>>,
    new_terminal: CaptionAction,
    close_window: CaptionAction,
}
impl CompactDock {
    pub fn new(
        skin: Rc<DockSkin>,
        caption: Rc<Cell<Option<NodeId>>>,
        new_terminal: CaptionAction,
        close_window: CaptionAction,
    ) -> Self {
        Self {
            skin,
            caption,
            new_terminal,
            close_window,
        }
    }
}

/// A vertical split keeps the top child; a horizontal split keeps the right.
/// Follow the tree rather than last frame's pixel bounds, including during resize.
fn upper_right(node: &PaneNode) -> NodeId {
    match node.kind() {
        PaneRef::Tabs { .. } => node.id(),
        PaneRef::Split { axis, children, .. } => children
            .get(if axis == Axis::Horizontal {
                children.len().saturating_sub(1)
            } else {
                0
            })
            .map(upper_right)
            .unwrap_or(node.id()),
    }
}
pub fn caption_node(area: &DockArea) -> Option<NodeId> {
    area.zoomed_group().or_else(|| {
        area.layout(DockPlacement::Center)
            .map(|tree| upper_right(tree.root()))
    })
}

impl DockAreaRenderer for CompactDock {
    fn frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.frame(window, cx)
    }
    fn center_frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.center_frame(window, cx)
    }
    fn split_frame(
        &self,
        node: NodeId,
        axis: Axis,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.skin.split_frame(node, axis, window, cx)
    }
    fn render_split_handle(
        &self,
        handle: &gpui_kit::base::ResizeHandleContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.skin.render_split_handle(handle, window, cx)
    }
    fn render_dock(
        &self,
        dock: &DockContext,
        content: AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.skin.render_dock(dock, content, window, cx)
    }
    fn build_placeholder(
        &self,
        state: &PanelState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<dyn BasePanelView>> {
        self.skin.build_placeholder(state, window, cx)
    }
    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        Rc::new(CompactTabs {
            inner: self.skin.tab_group_renderer(),
            caption: self.caption.clone(),
            new_terminal: self.new_terminal.clone(),
            close_window: self.close_window.clone(),
        })
    }
}
struct CompactTabs {
    inner: Rc<dyn TabGroupRenderer>,
    caption: Rc<Cell<Option<NodeId>>>,
    new_terminal: CaptionAction,
    close_window: CaptionAction,
}
impl TabGroupRenderer for CompactTabs {
    fn frame(&self, group: &TabGroupContext, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.inner.frame(group, window, cx)
    }
    fn content_frame(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.inner.content_frame(group, window, cx)
    }
    fn render_tab_bar(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let tabs = self.inner.render_tab_bar(group, window, cx);
        if self.caption.get() != Some(group.node()) {
            return tabs;
        }
        div()
            .id("caption-row")
            .test_support()
            .flex()
            .h(px(HEADER_HEIGHT))
            .flex_shrink_0()
            .w_full()
            .bg(rgb(0x181f29))
            .child(div().flex_1().min_w_0().overflow_hidden().child(tabs))
            .child(
                div()
                    .w(px(16.))
                    .h_full()
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                Button::new("new-terminal")
                    .text_color(rgb(0x79b8ed))
                    .icon(IconName::Plus)
                    .accessibility_label("New Terminal")
                    .tooltip("New Terminal (Ctrl+Shift+T)")
                    .small()
                    .ghost()
                    .w(px(34.))
                    .h(px(HEADER_HEIGHT))
                    .on_click({
                        let action = self.new_terminal.clone();
                        move |_, window, cx| action(window, cx)
                    }),
            )
            .child(window_control(
                "window-minimize",
                IconName::WindowMinimize,
                WindowControlArea::Min,
                self.close_window.clone(),
            ))
            .child(window_control(
                "window-maximize",
                if window.is_maximized() {
                    IconName::WindowRestore
                } else {
                    IconName::WindowMaximize
                },
                WindowControlArea::Max,
                self.close_window.clone(),
            ))
            .child(window_control(
                "window-close",
                IconName::WindowClose,
                WindowControlArea::Close,
                self.close_window.clone(),
            ))
            .into_any_element()
    }
    fn render_active_panel(
        &self,
        panel: AnyView,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.inner.render_active_panel(panel, group, window, cx)
    }
    fn render_drop_indicator(
        &self,
        indicator: DropIndicator,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.inner.render_drop_indicator(indicator, window, cx)
    }
    fn render_empty(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.inner.render_empty(group, window, cx)
    }
}
fn window_control(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    close_window: CaptionAction,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .w(px(34.))
        .h_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .hover(move |style| {
            style.bg(rgb(if id == "window-close" {
                0xb83242
            } else {
                0x354253
            }))
        })
        .when(cfg!(windows), |view| view.window_control_area(area))
        .when(!cfg!(windows), |view| {
            view.on_click(move |_, window, cx| match id {
                "window-minimize" => window.minimize_window(),
                "window-maximize" => window.zoom_window(),
                _ => close_window(window, cx),
            })
        })
        .child(Icon::new(icon).small())
}
