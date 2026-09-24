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
- File-tree creation, rename, confirmed deletion, path copying, Explorer file drops, terminal paste, and drag-to-terminal paths
- Git status detection across multiple nested repositories
- VS Code-style Git changes view with confirmed discard actions and side-by-side working-tree diffs
- Unified file and terminal tabs, organized in Orca-style pane-local tab bars
- Multiple independent WSL terminals with ANSI colors, Korean text, IME input, and visible start paths
- On-demand, read-only WSL diagnostics from the status bar (workspace and host disk, running distributions, top processes, Docker containers, available memory, and Git)
- Repeatable horizontal and vertical tab-pane splits with independent tabs and resizable dividers
- WSL-aware CLI arguments and Linux-to-UNC path mapping
- Headless behavioral Harness for editor isolation, multiple terminals, docking layouts, Git, and UI regressions

## Actively Evolving

Araseo is an active personal project. More editor, terminal, Git, workspace,
and quality-of-life features will continue to be added while keeping startup,
resource usage, and the overall interface lightweight.

To split a pane, open another file or terminal tab and drag its tab to the
left, right, top, or bottom edge of the target pane. Drop in the center to
move the tab into that pane. You can repeat this on any pane; closing or moving
its last tab removes the empty pane. Drag a divider to resize its two sides.

Workspace startup, file opening, tree scanning, and emoji preparation run on
background workers. Rapid file selections keep the latest requested file;
activating or closing a tab cancels a pending open. While typing, text is shown
immediately in plain color and syntax colors return after a 120 ms pause plus
background processing. Unchanged line numbers and editor buffers are retained.
Continuous terminal output is processed in bounded batches to let input run.

A manual presentation microbenchmark is available with
`cargo test --manifest-path ui-harness/Cargo.toml benchmark_edit_presentation -- --ignored --nocapture`.
It compares the old per-edit highlighting work with the deferred input path;
it excludes document edits, rendering, and eventual background highlighting.

The product specification is in [docs/PRD.md](docs/PRD.md).

## Installation

From WSL, install the newest release with:

```bash
curl -fsSL https://github.com/mincheol-lee/araseo/releases/latest/download/install.sh | sh
araseo .
```

The same command updates an existing installation. It installs the native
executable under Windows LocalAppData and the `araseo` command under
`~/.local/bin`. See [INSTALL.md](INSTALL.md) for pinned versions, manual
installation, checksum verification, and removal instructions.

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

For a development checkout, put `scripts/araseo` on the WSL `PATH`; the launcher
will use `dist/araseo.exe`. If the executable is elsewhere, set `ARASEO_EXE` in
WSL to its interop path:

```bash
export ARASEO_EXE=/mnt/c/Tools/Araseo/araseo.exe
araseo .
```

For UI development on Linux/WSLg, running `cargo run -- .` opens the current Linux directory and uses a Unix PTY instead of ConPTY.

Ubuntu development builds require Fontconfig and pkg-config headers:

```bash
sudo apt install pkg-config libfontconfig1-dev libxkbcommon-dev libwayland-dev
```

Run the headless verification harness and release-script tests after a change:

```bash
./scripts/verify
```

It executes the production editor/document code, file tree, workspace path
checks, terminal key and Korean character-width checks, and a real multi-repo
Git/inotify integration scenario. It does not need the Linux Slint/fontconfig
development packages.

Set `ARASEO_UI_SNAPSHOT_DIR` to have the UI Harness also write full-frame PNG
snapshots of the single-editor, split-pane, and single-terminal layouts:

```bash
ARASEO_UI_SNAPSHOT_DIR=/tmp/araseo-ui-snapshots ./scripts/verify
```

Before distributing a Windows build, also compile every test and the Slint UI
for the Windows target:

```bash
./scripts/verify --windows
```

`scripts/build-windows.ps1` runs the headless harness automatically before it
creates `dist/araseo.exe`. Pushing a version tag such as `v0.1.9` runs both
verification modes, builds the Windows executable, checks that the tag matches
the Cargo package version, and publishes the GitHub Release assets. Tags with a
pre-release suffix, such as `v0.2.0-beta.1`, create a GitHub pre-release.

Use **Aa** in the status bar to change terminal, file viewer, and file list font sizes and brightness independently. **Reset** restores the Orca-matched defaults (14, 14, and 12 px at 120% brightness). Settings are saved in `%LOCALAPPDATA%/araseo/fonts.conf` on Windows or `$XDG_CONFIG_HOME/araseo/fonts.conf` (default `~/.config/araseo/fonts.conf`) on Linux.

## Support

If Araseo is useful to you and you'd like to support its development, you can
[buy me a coffee](https://buymeacoffee.com/mclee). Araseo remains free to use.
