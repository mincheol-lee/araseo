use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub kind: String,
    pub markup: String,
}

pub fn parse(source: &str) -> Vec<Block> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let mut blocks = Vec::new();
    let mut current: Option<Block> = None;
    let mut list_depth: usize = 0;
    let mut list_number: Vec<Option<u64>> = Vec::new();
    let mut in_item = false;

    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                finish(&mut current, &mut blocks);
                current = Some(Block {
                    kind: format!("h{}", level as u8),
                    markup: String::new(),
                });
            }
            Event::End(TagEnd::Heading(_)) => finish(&mut current, &mut blocks),
            Event::Start(Tag::Paragraph) if current.is_none() => {
                current = Some(Block {
                    kind: "paragraph".into(),
                    markup: String::new(),
                });
            }
            Event::End(TagEnd::Paragraph) if !in_item => finish(&mut current, &mut blocks),
            Event::Start(Tag::CodeBlock(_)) => {
                finish(&mut current, &mut blocks);
                current = Some(Block {
                    kind: "code".into(),
                    markup: String::new(),
                });
            }
            Event::End(TagEnd::CodeBlock) => finish(&mut current, &mut blocks),
            Event::Start(Tag::List(number)) => {
                list_depth += 1;
                list_number.push(number);
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                list_number.pop();
            }
            Event::Start(Tag::Item) => {
                finish(&mut current, &mut blocks);
                in_item = true;
                let marker = if let Some(Some(number)) = list_number.last_mut() {
                    let marker = format!("{number}. ");
                    *number += 1;
                    marker
                } else {
                    "• ".into()
                };
                current = Some(Block {
                    kind: "list".into(),
                    markup: format!("{}{}", "  ".repeat(list_depth.saturating_sub(1)), marker),
                });
            }
            Event::End(TagEnd::Item) => {
                in_item = false;
                finish(&mut current, &mut blocks);
            }
            Event::Start(Tag::TableRow) => {
                finish(&mut current, &mut blocks);
                current = Some(Block {
                    kind: "table".into(),
                    markup: String::new(),
                });
            }
            Event::End(TagEnd::TableRow) => finish(&mut current, &mut blocks),
            Event::End(TagEnd::TableCell) => {
                if let Some(block) = &mut current {
                    block.markup.push_str("    ");
                }
            }
            Event::Start(Tag::Strong) => append(&mut current, "**"),
            Event::End(TagEnd::Strong) => append(&mut current, "**"),
            Event::Start(Tag::Emphasis) => append(&mut current, "*"),
            Event::End(TagEnd::Emphasis) => append(&mut current, "*"),
            Event::Start(Tag::Strikethrough) => append(&mut current, "~~"),
            Event::End(TagEnd::Strikethrough) => append(&mut current, "~~"),
            Event::Text(text) => append(&mut current, &text),
            Event::Code(code) => {
                append(&mut current, "`");
                append(&mut current, &code);
                append(&mut current, "`");
            }
            Event::SoftBreak | Event::HardBreak => append(&mut current, "\n"),
            Event::TaskListMarker(checked) => {
                append(&mut current, if checked { "☑ " } else { "☐ " })
            }
            Event::Rule => blocks.push(Block {
                kind: "rule".into(),
                markup: "────────────────".into(),
            }),
            _ => {}
        }
    }
    finish(&mut current, &mut blocks);
    blocks
}

fn append(current: &mut Option<Block>, text: &str) {
    if current.is_none() {
        *current = Some(Block {
            kind: "paragraph".into(),
            markup: String::new(),
        });
    }
    current.as_mut().unwrap().markup.push_str(text);
}

fn finish(current: &mut Option<Block>, blocks: &mut Vec<Block>) {
    if let Some(block) = current.take() {
        if !block.markup.trim().is_empty() {
            blocks.push(block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_heading_paragraph_list_and_code() {
        let blocks = parse(
            "# Title\n\nSome **bold** text.\n\n- first\n- second\n\n```rust\nlet x = 1;\n```\n",
        );
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.kind.as_str())
                .collect::<Vec<_>>(),
            ["h1", "paragraph", "list", "list", "code"]
        );
        assert_eq!(blocks[1].markup, "Some **bold** text.");
        assert!(blocks[4].markup.contains("let x = 1;"));
    }
}
