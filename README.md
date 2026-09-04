# Araseo

**A lightweight, Rust-native code editor built for WSL.**

Araseo brings the essentials of a coding IDE into one fast, focused desktop
application: browse a workspace, open and edit files, inspect Git changes, and
run WSL commands or coding agents without leaving the window.

Araseo is built from the ground up in **Rust**. It favors a small native
application and a focused feature set over the memory and complexity of a
full-scale IDE.

## Built with Rust

- **Rust application core** for workspace, document, Git, and terminal logic
- **Slint native UI** instead of a browser-based desktop shell
- **Real PTY and VT100 rendering** for interactive WSL terminal applications
- **Single Windows executable** with a WSL-aware command-line launcher

## Current Features

- Lazy, expandable file tree with context-aware icons
- Git status detection across multiple nested repositories
- Tabbed UTF-8 editor with syntax highlighting, undo/redo, and conflict detection
- Interactive WSL terminal with ANSI colors, Korean text, and IME input
- WSL-aware CLI arguments and Linux-to-UNC path mapping
- Headless behavioral Harness for editor, terminal, Git, and UI regressions

## Actively Evolving

Araseo is an active personal project. More editor, terminal, Git, workspace,
and quality-of-life features will continue to be added while keeping startup,
resource usage, and the overall interface lightweight.

The product specification is in [docs/PRD.md](docs/PRD.md).

## Development

Prerequisites:

- Rust stable
- Windows 10 1809+ or Windows 11
- WSL2 with an Ubuntu distribution

Build the native application on Windows:

```powershell
.\scripts\build-windows.ps1
```

This produces `dist/araseo.exe`.

Install `target/release/araseo.exe` somewhere on the Windows `PATH`, then place `scripts/araseo` on the WSL `PATH`. If the executable is not named or located as expected, set `ARASEO_EXE` in WSL to its interop path:

```bash
export ARASEO_EXE=/mnt/c/Tools/Araseo/araseo.exe
araseo .
```

For UI development on Linux/WSLg, running `cargo run -- .` opens the current Linux directory and uses a Unix PTY instead of ConPTY.

Ubuntu development builds require Fontconfig and pkg-config headers:

```bash
sudo apt install pkg-config libfontconfig1-dev libxkbcommon-dev libwayland-dev
```

Run the headless verification harness after a change:

```bash
./scripts/verify
```

It executes the production editor/document code, file tree, workspace path
checks, terminal key and Korean character-width checks, and a real multi-repo
Git/inotify integration scenario. It does not need the Linux Slint/fontconfig
development packages.

Before distributing a Windows build, also compile every test and the Slint UI
for the Windows target:

```bash
./scripts/verify --windows
```

`scripts/build-windows.ps1` runs the headless harness automatically before it
creates `dist/araseo.exe`.
