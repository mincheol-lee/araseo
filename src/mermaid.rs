//! Offline Mermaid rendering with bounded inputs and raster allocation.
//! This module is called only by the Markdown preparation worker.
use crate::preview::ImagePixels;
use anyhow::{Context, Result, bail};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use std::sync::{Arc, OnceLock};

pub const MAX_DIAGRAMS: usize = 32;
pub const MAX_DIAGRAM_PIXELS: u64 = 2 * 1024 * 1024;
pub const MAX_DOCUMENT_PIXELS: u64 = 8 * 1024 * 1024;

/// Parse fenced blocks, including fences nested in lists/quotes. Ordinary code
/// and inline mentions of Mermaid are left to the normal Markdown renderer.
pub fn sources(markdown: &str) -> Vec<String> {
    let mut sources = Vec::new();
    let mut code = None;
    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if info
                    .split_whitespace()
                    .next()
                    .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid")) =>
            {
                code = Some(String::new());
            }
            Event::Text(text) if code.is_some() => code.as_mut().unwrap().push_str(&text),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(text) = code.take() {
                    sources.push(text.trim_end_matches('\n').to_owned());
                    if sources.len() == MAX_DIAGRAMS {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    sources
}

pub fn render(source: &str) -> Result<ImagePixels> {
    // A malformed third-party diagram must not take down the UI or leave its
    // preparation task permanently pending.
    std::panic::catch_unwind(|| render_inner(source)).unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "Mermaid renderer could not process this diagram"
        ))
    })
}

fn render_inner(source: &str) -> Result<ImagePixels> {
    if source.len() > 16 * 1024 || source.lines().count() > 256 {
        bail!("Mermaid diagram exceeds the preview limit (16 KiB / 256 lines)");
    }
    let parsed = mermaid_rs_renderer::parse_mermaid(source)?;
    if parsed.graph.nodes.len() > 128 || parsed.graph.edges.len() > 256 {
        bail!("Mermaid diagram exceeds the preview limit (128 nodes / 256 edges)");
    }
    let svg = mermaid_rs_renderer::render_with_options(
        source,
        mermaid_rs_renderer::RenderOptions {
            theme: mermaid_rs_renderer::Theme::dark(),
            ..Default::default()
        },
    )?;
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    let options = resvg::usvg::Options {
        fontdb: FONTS
            .get_or_init(|| {
                let mut fonts = resvg::usvg::fontdb::Database::new();
                fonts.load_system_fonts();
                Arc::new(fonts)
            })
            .clone(),
        // Diagram previews are self-contained. Do not resolve image paths or
        // data URLs embedded in diagram input.
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(&svg, &options)?;
    let size = tree.size();
    let scale = (2048. / size.width())
        .min(2048. / size.height())
        .min((MAX_DIAGRAM_PIXELS as f32 / (size.width() * size.height())).sqrt())
        .min(1.5);
    let width = (size.width() * scale).floor().max(1.) as u32;
    let height = (size.height() * scale).floor().max(1.) as u32;
    let mut pixels = resvg::tiny_skia::Pixmap::new(width, height)
        .context("cannot allocate Mermaid preview pixels")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    Ok(ImagePixels {
        width,
        height,
        rgba: pixels.take(),
        premultiplied: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_only_mermaid_fences_in_document_order() {
        let input = "# Title\n\n```rust\nlet mermaid = 1;\n```\n\n> ~~~mermaid\n> flowchart LR\n> A --> B\n> ~~~\n\n```Mermaid title\nsequenceDiagram\nA->>B: hello\n```\n";
        assert_eq!(
            sources(input),
            vec!["flowchart LR\nA --> B", "sequenceDiagram\nA->>B: hello"]
        );
        assert!(sources("`mermaid`\n\n    mermaid\n    A --> B").is_empty());
    }
    #[test]
    fn renders_real_flowcharts_and_sequence_diagrams_offline() {
        for source in [
            "flowchart TD\nA[Start] --> B{Decision}\nB -->|Yes| C[Done]\nB -->|No| A",
            "sequenceDiagram\nAlice->>Bob: Hello\nBob-->>Alice: Hi",
            "flowchart LR\nA[시작] --> B[완료]",
        ] {
            let pixels = render(source).unwrap();
            assert!(pixels.width > 20 && pixels.height > 20);
            assert!(u64::from(pixels.width) * u64::from(pixels.height) <= MAX_DIAGRAM_PIXELS);
            assert_eq!(
                pixels.rgba.len(),
                pixels.width as usize * pixels.height as usize * 4
            );
            assert!(pixels.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
        }
    }
    #[test]
    fn invalid_and_oversized_diagrams_return_errors() {
        assert!(render("this is not a Mermaid diagram").is_err());
        assert!(render(&format!("flowchart TD\n{}", "A-->B;".repeat(4000))).is_err());
        let large = format!(
            "flowchart TD\n{}",
            (0..130)
                .map(|i| format!("A{i}-->A{};", i + 1))
                .collect::<String>()
        );
        assert!(render(&large).is_err());
    }
}
