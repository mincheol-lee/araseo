use slint::StyledText;
use std::path::Path;

// Highlighting creates a StyledText span for every token. Generated dependency
// manifests can contain tens of thousands of tiny JSON tokens, so their
// highlighted representation is much more expensive than the source text.
const MAX_HIGHLIGHT_BYTES: usize = 256 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 4_000;
const MAX_HIGHLIGHT_COMPLEXITY: usize = 24_000;

const GENERATED_LOCK_FILES: &[&str] = &[
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "composer.lock",
    "gemfile.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "pdm.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "uv.lock",
    "yarn.lock",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Language {
    Rust,
    JavaScript,
    Python,
    Json,
    Markup,
    Css,
    Config,
    Shell,
    CLike,
    Markdown,
    Sql,
}

#[derive(Clone, Copy)]
enum TokenKind {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
    Operator,
    Property,
}

pub fn highlighted(path: &Path, text: &str) -> Option<StyledText> {
    if !should_highlight(path, text) {
        return None;
    }
    let language = language_for(path)?;
    StyledText::from_markdown(&highlight_markup(language, text)).ok()
}

pub fn should_highlight(path: &Path, text: &str) -> bool {
    let Some(file_name) = path.file_name() else {
        return false;
    };
    let file_name = file_name.to_string_lossy().to_ascii_lowercase();
    if GENERATED_LOCK_FILES.contains(&file_name.as_str()) || text.len() > MAX_HIGHLIGHT_BYTES {
        return false;
    }

    let mut lines = 1;
    let mut complexity = 0;
    for byte in text.bytes() {
        if byte == b'\n' {
            lines += 1;
            if lines > MAX_HIGHLIGHT_LINES {
                return false;
            }
        }
        if byte.is_ascii_whitespace() || b"{}[]()=:+-*/%<>!&|.?@,".contains(&byte) {
            complexity += 1;
            if complexity > MAX_HIGHLIGHT_COMPLEXITY {
                return false;
            }
        }
    }

    language_for(path).is_some()
}

fn language_for(path: &Path) -> Option<Language> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if matches!(name.as_str(), "dockerfile" | "makefile" | "justfile") {
        return Some(if name == "dockerfile" {
            Language::Shell
        } else {
            Language::Config
        });
    }
    match path.extension()?.to_string_lossy().to_ascii_lowercase().as_str() {
        "rs" => Some(Language::Rust),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => Some(Language::JavaScript),
        "py" | "pyw" => Some(Language::Python),
        "json" | "jsonc" => Some(Language::Json),
        "html" | "htm" | "xml" | "svg" | "slint" => Some(Language::Markup),
        "css" | "scss" | "sass" | "less" => Some(Language::Css),
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "env" => {
            Some(Language::Config)
        }
        "sh" | "bash" | "zsh" | "fish" => Some(Language::Shell),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "java" | "go" | "cs"
        | "swift" | "kt" | "kts" | "dart" => Some(Language::CLike),
        "md" | "markdown" | "mdx" => Some(Language::Markdown),
        "sql" => Some(Language::Sql),
        _ => None,
    }
}

fn highlight_markup(language: Language, text: &str) -> String {
    let mut output = String::with_capacity(text.len().saturating_mul(3));
    let mut block_comment = false;
    for line_with_ending in text.split_inclusive('\n') {
        let (line, newline) = line_with_ending
            .strip_suffix('\n')
            .map_or((line_with_ending, false), |line| (line, true));
        highlight_line(language, line, &mut block_comment, &mut output);
        if line.is_empty() {
            push_token(&mut output, "\u{200b}", TokenKind::Plain);
        }
        if newline {
            output.push('\n');
        }
    }
    if text.is_empty() {
        push_token(&mut output, "\u{200b}", TokenKind::Plain);
    }
    output
}

fn highlight_line(
    language: Language,
    line: &str,
    block_comment: &mut bool,
    output: &mut String,
) {
    if language == Language::Markdown && line.trim_start().starts_with('#') {
        push_token(output, line, TokenKind::Keyword);
        return;
    }

    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if *block_comment {
            let end = line[index..].find("*/").map_or(bytes.len(), |offset| index + offset + 2);
            push_token(output, &line[index..end], TokenKind::Comment);
            index = end;
            if index < bytes.len() || line[..index].ends_with("*/") {
                *block_comment = false;
            }
            continue;
        }

        if supports_block_comments(language) && line[index..].starts_with("/*") {
            let end = line[index + 2..]
                .find("*/")
                .map(|offset| index + 2 + offset + 2);
            match end {
                Some(end) => {
                    push_token(output, &line[index..end], TokenKind::Comment);
                    index = end;
                }
                None => {
                    push_token(output, &line[index..], TokenKind::Comment);
                    *block_comment = true;
                    break;
                }
            }
            continue;
        }

        if let Some(marker) = line_comment(language)
            && line[index..].starts_with(marker)
        {
            push_token(output, &line[index..], TokenKind::Comment);
            break;
        }
        if language == Language::Markup && line[index..].starts_with("<!--") {
            let end = line[index + 4..]
                .find("-->")
                .map_or(bytes.len(), |offset| index + 4 + offset + 3);
            push_token(output, &line[index..end], TokenKind::Comment);
            index = end;
            continue;
        }

        let character = line[index..].chars().next().unwrap();
        if matches!(character, '\'' | '"' | '`') {
            let end = quoted_end(line, index, character);
            let kind = if language == Language::Json
                && line[end..].trim_start().starts_with(':')
            {
                TokenKind::Property
            } else {
                TokenKind::String
            };
            push_token(output, &line[index..end], kind);
            index = end;
            continue;
        }
        if character.is_ascii_digit() {
            let end = take_while(line, index, |value| {
                value.is_ascii_alphanumeric() || matches!(value, '.' | '_' | 'x' | 'X')
            });
            push_token(output, &line[index..end], TokenKind::Number);
            index = end;
            continue;
        }
        if is_identifier_start(character) {
            let end = take_while(line, index, is_identifier_continue);
            let word = &line[index..end];
            let rest = line[end..].trim_start();
            let previous = line[..index].trim_end().chars().next_back();
            let kind = if is_keyword(language, word) {
                TokenKind::Keyword
            } else if is_type_name(language, word) {
                TokenKind::Type
            } else if rest.starts_with('(') {
                TokenKind::Function
            } else if is_property(language, previous, rest) {
                TokenKind::Property
            } else {
                TokenKind::Plain
            };
            push_token(output, word, kind);
            index = end;
            continue;
        }
        let end = index + character.len_utf8();
        push_token(
            output,
            &line[index..end],
            if "{}[]()=:+-*/%<>!&|.?@".contains(character) {
                TokenKind::Operator
            } else {
                TokenKind::Plain
            },
        );
        index = end;
    }
}

fn line_comment(language: Language) -> Option<&'static str> {
    match language {
        Language::Rust | Language::JavaScript | Language::CLike => Some("//"),
        Language::Python | Language::Config | Language::Shell => Some("#"),
        Language::Sql => Some("--"),
        _ => None,
    }
}

fn supports_block_comments(language: Language) -> bool {
    matches!(
        language,
        Language::Rust | Language::JavaScript | Language::CLike | Language::Css | Language::Sql
    )
}

fn quoted_end(line: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    for (offset, character) in line[start + quote.len_utf8()..].char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            return start + quote.len_utf8() + offset + character.len_utf8();
        }
    }
    line.len()
}

fn take_while(line: &str, start: usize, predicate: impl Fn(char) -> bool) -> usize {
    let mut end = start;
    for character in line[start..].chars() {
        if !predicate(character) {
            break;
        }
        end += character.len_utf8();
    }
    end
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn is_property(language: Language, previous: Option<char>, rest: &str) -> bool {
    match language {
        Language::Config => rest.starts_with('=') || rest.starts_with(':'),
        Language::Css => rest.starts_with(':'),
        Language::Markup => previous == Some('<') || previous == Some('/'),
        _ => false,
    }
}

fn is_type_name(language: Language, word: &str) -> bool {
    matches!(language, Language::Rust | Language::CLike | Language::JavaScript)
        && word.chars().next().is_some_and(char::is_uppercase)
}

fn is_keyword(language: Language, word: &str) -> bool {
    let common = ["true", "false", "null", "none", "self", "this", "super"];
    if common.contains(&word.to_ascii_lowercase().as_str()) {
        return true;
    }
    match language {
        Language::Rust => [
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
            "enum", "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match",
            "mod", "move", "mut", "pub", "ref", "return", "static", "struct", "trait",
            "type", "unsafe", "use", "where", "while",
        ]
        .contains(&word),
        Language::JavaScript => [
            "async", "await", "break", "case", "catch", "class", "const", "continue",
            "debugger", "default", "delete", "do", "else", "export", "extends", "finally",
            "for", "from", "function", "if", "import", "in", "instanceof", "interface", "let",
            "new", "of", "return", "switch", "throw", "try", "typeof", "var", "while", "yield",
        ]
        .contains(&word),
        Language::Python => [
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
            "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
            "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try",
            "while", "with", "yield",
        ]
        .contains(&word),
        Language::CLike => [
            "break", "case", "class", "const", "continue", "default", "do", "else", "enum",
            "extends", "final", "for", "func", "if", "import", "interface", "namespace", "new",
            "package", "private", "protected", "public", "return", "static", "struct", "switch",
            "throw", "try", "type", "using", "var", "void", "while",
        ]
        .contains(&word),
        Language::Shell => [
            "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function",
            "if", "in", "local", "then", "until", "while",
        ]
        .contains(&word),
        Language::Sql => [
            "alter", "and", "as", "asc", "by", "create", "delete", "desc", "distinct", "drop",
            "from", "group", "having", "insert", "into", "join", "limit", "not", "null", "on",
            "or", "order", "select", "set", "table", "union", "update", "values", "where",
        ]
        .contains(&word.to_ascii_lowercase().as_str()),
        Language::Markup => [
            "component", "export", "import", "inherits", "property", "callback", "function",
            "if", "for", "in", "states", "animate",
        ]
        .contains(&word),
        _ => false,
    }
}

fn push_token(output: &mut String, text: &str, kind: TokenKind) {
    let color = match kind {
        TokenKind::Plain => "#abb2bf",
        TokenKind::Keyword => "#c678dd",
        TokenKind::String => "#98c379",
        TokenKind::Comment => "#5c6370",
        TokenKind::Number => "#d19a66",
        TokenKind::Function => "#61afef",
        TokenKind::Type => "#e5c07b",
        TokenKind::Operator => "#56b6c2",
        TokenKind::Property => "#e06c75",
    };
    output.push_str("<font color=\"");
    output.push_str(color);
    output.push_str("\">");
    escape_markup(text, output);
    output.push_str("</font>");
}

fn escape_markup(text: &str, output: &mut String) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\\' | '*' | '_' | '`' | '[' | ']' | '(' | ')' | '#' | '+' | '-' | '!' | '|'
            | '~' => {
                output.push('\\');
                output.push(character);
            }
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_extensions() {
        assert_eq!(language_for(Path::new("main.rs")), Some(Language::Rust));
        assert_eq!(language_for(Path::new("app.tsx")), Some(Language::JavaScript));
        assert_eq!(language_for(Path::new("notes.txt")), None);
    }

    #[test]
    fn colors_keywords_strings_comments_and_json_keys() {
        let rust = highlight_markup(Language::Rust, "pub fn main() { // hello\n}");
        assert!(rust.contains("#c678dd\">pub"));
        assert!(rust.contains("#61afef\">main"));
        assert!(rust.contains("#5c6370\">// hello"));

        let json = highlight_markup(Language::Json, "{\"name\": 42}");
        assert!(json.contains("#e06c75\">&quot;name&quot;"));
        assert!(json.contains("#d19a66\">42"));
    }

    #[test]
    fn skips_generated_lock_files_and_expensive_sources() {
        assert!(!should_highlight(
            Path::new("package-lock.json"),
            "{\"small\": true}"
        ));
        assert!(!should_highlight(
            Path::new("nested/Cargo.lock"),
            "package = \"araseo\""
        ));

        let too_many_lines = "let value = 1;\n".repeat(MAX_HIGHLIGHT_LINES + 1);
        assert!(!should_highlight(Path::new("generated.js"), &too_many_lines));

        let too_many_tokens = "{},".repeat(MAX_HIGHLIGHT_COMPLEXITY);
        assert!(!should_highlight(Path::new("generated.json"), &too_many_tokens));
        assert!(should_highlight(Path::new("small.json"), "{\"small\": true}"));
    }
}
