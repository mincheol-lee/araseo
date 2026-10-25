//! Preserve independent editor brightness without changing menu/tree colors.
//! The Kit editor installs its native editing behavior during layout; this
//! element then projects Araseo's colors through the public editor style API.
use gpui_kit::base::input::{HighlightStyleResolver, InputEditorStyle};
use gpui_kit::component::{
    Theme,
    highlighter::HighlightTheme,
    input::{Editor, EditorState},
};
use gpui_kit::{prelude::*, *};
use std::sync::Arc;

pub fn font_color(color: Hsla, brightness: i32) -> Hsla {
    let mut color = Rgba::from(color);
    let factor = brightness as f32 / 120.;
    color.r = (color.r * factor).min(1.);
    color.g = (color.g * factor).min(1.);
    color.b = (color.b * factor).min(1.);
    color.into()
}
struct Highlights {
    theme: Arc<HighlightTheme>,
    brightness: i32,
}
impl HighlightStyleResolver for Highlights {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        let mut style = self.theme.style(name)?;
        style.color = style.color.map(|color| font_color(color, self.brightness));
        Some(style)
    }
}
pub struct EditorSurface {
    editor: Entity<EditorState>,
    inner: AnyElement,
    brightness: i32,
}
impl EditorSurface {
    pub fn new(editor: &Entity<EditorState>, font_size: f32, brightness: i32) -> Self {
        Self {
            editor: editor.clone(),
            inner: Editor::new(editor)
                .context_menu(|menu, _, _| {
                    use crate::gpui_navigation::*;
                    use gpui_kit::component::input::{Copy, Cut, GoToDefinition, Paste, SelectAll};
                    menu.menu("Go to Definition · F12", Box::new(GoToDefinition))
                        .menu("Find Definitions · Alt+F12", Box::new(FindDefinitions))
                        .menu("Find All References · Shift+F12", Box::new(FindReferences))
                        .menu("Show Call Hierarchy · Shift+Alt+H", Box::new(ShowCallers))
                        .menu(
                            "Search Project Functions · Ctrl+T",
                            Box::new(SearchFunctions),
                        )
                        .separator()
                        .menu("Back · Alt+Left", Box::new(NavigateBack))
                        .menu("Forward · Alt+Right", Box::new(NavigateForward))
                        .separator()
                        .menu("Cut", Box::new(Cut))
                        .menu("Copy", Box::new(Copy))
                        .menu("Paste", Box::new(Paste))
                        .menu("Select All", Box::new(SelectAll))
                })
                .h_full()
                .text_size(px(font_size))
                .bordered(false)
                .into_any_element(),
            brightness,
        }
    }
}
impl IntoElement for EditorSurface {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for EditorSurface {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let layout = self.inner.request_layout(window, cx);
        let theme = Theme::global(cx);
        let style = InputEditorStyle {
            foreground: font_color(theme.foreground, self.brightness),
            muted_foreground: font_color(theme.muted_foreground, self.brightness),
            background: theme
                .highlight_theme
                .style
                .editor_background
                .unwrap_or(theme.background),
            border: theme.border,
            selection: theme.selection,
            caret: theme.caret,
            highlight_styles: Arc::new(Highlights {
                theme: theme.highlight_theme.clone(),
                brightness: self.brightness,
            }),
            editor_invisible: theme.highlight_theme.style.editor_invisible,
            editor_active_line: theme.highlight_theme.style.editor_active_line,
            editor_gutter_background: theme.highlight_theme.style.editor_gutter_background,
            ..Default::default()
        };
        self.editor
            .update(cx, |editor, _| editor.set_editor_style(style));
        (layout, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(window, cx);
    }
}
