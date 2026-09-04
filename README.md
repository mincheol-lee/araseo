# Araseo

Araseo is a lightweight Windows desktop editor for WSL workspaces. It combines a file tree, a basic tabbed editor, Git status markers, and an interactive WSL terminal in one Slint window.

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

## Current MVP

- Lazy, expandable file tree with Git porcelain-v2 `M` and `?` markers
- UTF-8 files up to 2 MiB, tabbed editing, line numbers, dirty state, save conflict detection
- A real PTY parsed into a VT100 screen, suitable for bash and interactive tools
- WSL-aware CLI arguments and Linux-to-UNC path mapping

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
