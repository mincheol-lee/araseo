//! Headless verification harness for Araseo's production core modules.
//!
//! The modules are loaded from `src/` rather than copied, so these tests always
//! exercise the same code that is compiled into `araseo.exe` without requiring a
//! Linux graphical/Slint toolchain.

#[path = "../../src/background.rs"]
pub mod background;
#[path = "../../src/diagnostics_log.rs"]
pub mod diagnostics_log;
#[path = "../../src/document.rs"]
pub mod document;
#[path = "../../src/git.rs"]
pub mod git;
#[path = "../../src/markdown_preview.rs"]
pub mod markdown_preview;
#[path = "../../src/preview.rs"]
pub mod preview;
#[path = "../../src/tabs.rs"]
pub mod tabs;
#[path = "../../src/terminal.rs"]
pub mod terminal;
#[path = "../../src/tree.rs"]
pub mod tree;
#[path = "../../src/window_activity.rs"]
pub mod window_activity;
#[path = "../../src/workspace.rs"]
pub mod workspace;
#[path = "../../src/wsl_diagnostics.rs"]
pub mod wsl_diagnostics;

#[path = "../../src/appearance.rs"]
pub mod appearance;
#[path = "../../src/session.rs"]
pub mod session;
#[path = "../../src/workspace_history.rs"]
pub mod workspace_history;

#[path = "../../src/terminal_input.rs"]
pub mod terminal_input;

#[path = "../../src/process_job.rs"]
pub mod process_job;

#[path = "../../src/document_io.rs"]
pub mod document_io;

#[cfg(any(windows, test))]
#[path = "../../src/terminal_resize.rs"]
pub mod terminal_resize;
