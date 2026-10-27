//! Markdown view with offline, debounced Mermaid preparation off the UI thread.
use crate::{gpui_panel::render_image, mermaid};
use gpui_kit::component::text::{
    MarkdownExtensions, MarkdownNode, TextView, TextViewState, markdown_ast,
};
use gpui_kit::{base::TestSupportExt, prelude::*, *};
use std::{collections::HashMap, sync::Arc, time::Duration};

#[derive(Clone)]
struct Diagram {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
}
type Diagrams = HashMap<String, Result<Diagram, String>>;

pub struct MarkdownPreview {
    text: String,
    state: Entity<TextViewState>,
    view: TextView,
    cache: Arc<Diagrams>,
    generation: u64,
    task: Option<Task<()>>,
}
impl MarkdownPreview {
    pub fn new(text: &str, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| TextViewState::markdown("", cx));
        let mut this = Self {
            text: String::new(),
            view: TextView::new(&state),
            state,
            cache: Arc::new(HashMap::new()),
            generation: 0,
            task: None,
        };
        this.set_text(text, cx);
        this
    }
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.text == text && self.generation != 0 {
            return;
        }
        self.text = text.to_owned();
        self.generation = self.generation.wrapping_add(1);
        self.task = None;
        // Preserve immediate rendering for ordinary Markdown. Only documents
        // that may contain Mermaid need diagram preparation/debouncing.
        if !text
            .as_bytes()
            .windows(7)
            .any(|word| word.eq_ignore_ascii_case(b"mermaid"))
        {
            self.cache = Arc::new(HashMap::new());
            self.state.update(cx, |state, cx| state.set_text(text, cx));
            self.view = TextView::new(&self.state);
            cx.notify();
            return;
        }
        let generation = self.generation;
        let text = self.text.clone();
        let cache = self.cache.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            // Repeated source edits coalesce before invoking layout/rasterization.
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let cache = cx
                .background_executor()
                .spawn(async move { prepare(&text, &cache) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.cache = Arc::new(cache);
                let cache = this.cache.clone();
                let extensions = MarkdownExtensions::default()
                    .parser_revision(generation)
                    .block_parser(move |node, _| {
                        let markdown_ast::Node::Code(code) = node else {
                            return None;
                        };
                        if !code
                            .lang
                            .as_deref()
                            .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid"))
                        {
                            return None;
                        }
                        let source = code.value.trim_end_matches('\n');
                        let data = cache.get(source).cloned().unwrap_or_else(|| {
                            Err("Too many Mermaid diagrams in this document (limit: 32)".into())
                        });
                        Some(
                            MarkdownNode::new("araseo-mermaid", data)
                                .text(code.value.clone())
                                .accessibility_label("Mermaid diagram"),
                        )
                    })
                    .block_renderer("araseo-mermaid", |node, _, _| diagram_element(node));
                this.state
                    .update(cx, |state, cx| state.set_text(&this.text, cx));
                this.view = TextView::new(&this.state).markdown_extensions(extensions);
                cx.notify();
            });
        }));
    }
    #[cfg(feature = "ui-tests")]
    pub fn rendered_text(&self, cx: &App) -> String {
        self.state.read(cx).rendered_text().as_str().to_owned()
    }
}
impl Render for MarkdownPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.view.clone()
    }
}
fn prepare(text: &str, previous: &Diagrams) -> Diagrams {
    let mut diagrams = HashMap::new();
    let mut pixels = 0;
    for source in mermaid::sources(text) {
        if diagrams.contains_key(&source) {
            continue;
        }
        // Resource-limit failures can disappear when other diagrams are removed.
        // Reuse successful images; retry errors for the new document generation.
        let result = previous
            .get(&source)
            .and_then(|result| result.as_ref().ok())
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| {
                mermaid::render(&source)
                    .map(|pixels| Diagram {
                        width: pixels.width,
                        height: pixels.height,
                        image: render_image(pixels),
                    })
                    .map_err(|error| error.to_string())
            });
        let result = match result {
            Ok(diagram) => {
                pixels += u64::from(diagram.width) * u64::from(diagram.height);
                if pixels > mermaid::MAX_DOCUMENT_PIXELS {
                    Err("Mermaid previews exceed this document's image memory limit".into())
                } else {
                    Ok(diagram)
                }
            }
            error => error,
        };
        diagrams.insert(source, result);
    }
    diagrams
}
fn diagram_element(node: &MarkdownNode) -> AnyElement {
    match node.data::<Result<Diagram, String>>() {
        Some(Ok(diagram)) => {
            let mut image = img(diagram.image.clone())
                .w_full()
                .max_w(px(diagram.width as f32));
            image.style().aspect_ratio = Some(diagram.width as f32 / diagram.height as f32);
            div()
                .id("mermaid-diagram")
                .test_support()
                .role(accesskit::Role::Image)
                .aria_label("Mermaid diagram")
                .w_full()
                .py_2()
                .child(image)
                .into_any_element()
        }
        result => div()
            .id("mermaid-error")
            .test_support()
            .w_full()
            .p_3()
            .child(format!(
                "Mermaid preview unavailable: {}",
                result
                    .and_then(|r| r.as_ref().err())
                    .map(String::as_str)
                    .unwrap_or("Unsupported diagram")
            ))
            .child(
                div()
                    .font_family("monospace")
                    .child(node.as_text().to_owned()),
            )
            .into_any_element(),
    }
}
