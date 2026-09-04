//! Headless verification harness for Araseo's production core modules.
//!
//! The modules are loaded from `src/` rather than copied, so these tests always
//! exercise the same code that is compiled into `araseo.exe` without requiring a
//! Linux graphical/Slint toolchain.

#[path = "../../src/workspace.rs"]
pub mod workspace;
#[path = "../../src/document.rs"]
pub mod document;
#[path = "../../src/tree.rs"]
pub mod tree;
#[path = "../../src/git.rs"]
pub mod git;
#[path = "../../src/terminal.rs"]
pub mod terminal;
#[path = "../../src/tabs.rs"]
pub mod tabs;
