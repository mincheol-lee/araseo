//! Test-only support for an isolated, non-submitting real Codex input fixture.
use crate::terminal::TerminalSession;
use std::path::{Path, PathBuf};
pub fn spawn(label: &str) -> (TerminalSession, PathBuf) {
    let linux_root = format!("/tmp/araseo-codex-{label}-{}", std::process::id());
    #[cfg(windows)]
    let host_root = PathBuf::from(format!(
        r"\\wsl.localhost\Ubuntu{}",
        linux_root.replace('/', r"\")
    ));
    #[cfg(not(windows))]
    let host_root = PathBuf::from(&linux_root);
    std::fs::create_dir_all(host_root.join("home")).unwrap();
    std::fs::create_dir_all(host_root.join("workspace")).unwrap();
    std::fs::write(
        host_root.join("home/config.toml"),
        format!(
            r#"model = "gpt-5"
model_provider = "araseo_fixture"
check_for_update_on_startup = false
[model_providers.araseo_fixture]
name = "Araseo input fixture"
base_url = "http://127.0.0.1:9/v1"
env_key = "ARASEO_FIXTURE_API_KEY"
wire_api = "responses"
requires_openai_auth = false
[projects."{linux_root}/workspace"]
trust_level = "trusted"
"#
        ),
    )
    .unwrap();
    let mut terminal =
        TerminalSession::spawn("Ubuntu", Path::new(&format!("{linux_root}/workspace"))).unwrap();
    terminal.resize(40, 120);
    // CODEX_HOME is used only for the child application's documented isolated
    // configuration directory; the user's environment and credentials are untouched.
    // Disable activity hooks only in this fixture: a fresh home otherwise shows
    // the hook review modal after the composer has already rendered once.
    terminal.write(format!("CODEX_HOME='{linux_root}/home' ARASEO_FIXTURE_API_KEY=fixture ARASEO_CODEX_HOOKS='hooks={{}}' codex --no-daemon --sandbox read-only --ask-for-approval never\r").as_bytes());
    (terminal, host_root)
}
pub fn text(terminal: &TerminalSession) -> String {
    let mut rows = std::collections::BTreeMap::<i32, (i32, String)>::new();
    for cell in terminal.cells() {
        let (column, line) = rows.entry(cell.row).or_default();
        for _ in *column..cell.column {
            line.push(' ');
        }
        if cell.glyph.is_empty() {
            line.push(' ');
        } else {
            line.push_str(&cell.glyph);
        }
        *column = cell.column + cell.column_span.max(1);
    }
    rows.into_values()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}
