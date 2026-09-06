# Araseo Contributor Guide

This document records the architecture and engineering decisions that are easy
to miss when reading individual files. It is intended for maintainers, AI coding
agents, and people starting from a fork.

`docs/PRD.md` describes product direction. The production code under `src/`, the
Slint UI in `ui/app.slint`, and the Harness tests are the source of truth for
current behavior. Some older PRD details may describe an intended design rather
than the implementation that ships today.

## Project in One Paragraph

Araseo is a lightweight native Windows editor for projects that live inside
WSL2. The Windows process owns the window and editor UI. Project files, Git,
bash, and coding agents such as Codex remain inside the selected WSL
distribution. The primary user entry point is `araseo .` from a WSL shell; the
shell wrapper passes the distribution name and absolute Linux path to the
Windows executable.

The first supported environment is Windows 10/11 x64 with WSL2 Ubuntu. Linux
builds are useful for development and the test harnesses, but local Windows
projects and non-WSL remote environments are not current product targets.

## Runtime Boundaries

```text
WSL distribution                              Windows process

scripts/araseo
  | --distro / --root / --file
  v
wsl.exe interop ----------------------------> AppWindow (Slint)
project files <------- \\wsl.localhost ------> Workspace / Document
git + inotify <------------ wsl.exe ---------> StatusMonitor worker
bash / Codex <---- script-created Linux PTY -> TerminalSession + vt100
```

Important consequences:

- Paths crossing the process boundary remain Linux paths in the CLI contract.
  `Workspace` is responsible for mapping them to
  `\\wsl.localhost\<distro>\...` paths on Windows.
- Git must run inside the selected WSL distribution. Windows Git can produce
  different path, permission, line-ending, and repository behavior.
- Do not build shell commands by concatenating user paths. Pass the distro,
  root, and file as separate process arguments.
- `Workspace::host_path` enforces the workspace boundary and rejects existing
  symlink targets that resolve outside it. Preserve this check in new file
  operations.

## Source Map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Application composition, Slint callbacks, UI synchronization, timers, and worker result routing |
| `src/workspace.rs` | Linux/host path mapping and workspace containment |
| `src/document.rs` | UTF-8 document state, undo/redo, line endings, atomic saves, disk revisions, and external refresh |
| `src/tree.rs` | Lazy flattened file tree, icons, and Git status decoration |
| `src/git.rs` | Recursive inotify monitor and WSL Git status worker |
| `src/tabs.rs` | Stable tab IDs and the pure two-group docking state machine |
| `src/terminal.rs` | WSL/Linux terminal processes, resizing, VT parsing, output wakeups, and key encoding |
| `src/highlight.rs` | Lightweight syntax highlighting and the policy for skipping expensive files |
| `src/emoji.rs` | System color-emoji lookup and rasterization for the file tree |
| `ui/app.slint` | The complete production UI and input routing |
| `harness/` | Headless tests that compile production core modules directly |
| `ui-harness/` | Software-rendered input, layout, clipboard, IME, and pixel regressions using the production Slint file |
| `vendor/i-slint-renderer-software/` | Narrowly patched Slint software renderer; see the dependency patch in `Cargo.toml` |
| `scripts/araseo` | WSL-facing launcher |
| `scripts/verify` | Required core, UI, and Windows verification entry point |

## Architecture Decisions

### Slint and the software renderer are intentional

Production explicitly selects the Slint `winit` backend and software renderer.
This keeps production behavior aligned with the UI Harness, including the
transparent editable text layer placed over syntax-highlighted text. Do not
change the renderer as a casual performance tweak; selection, IME, clipping,
font behavior, and Harness parity must be revalidated together.

The repository patches `i-slint-renderer-software` because upstream Slint 1.17
converts some off-screen glyph coordinates before clipping. The local patch
prevents large documents from overflowing that conversion. Keep the patch
narrow and revisit it when upgrading Slint.

Color emoji are rasterized through `swash` and supplied to Slint as images.
Plain font fallback is not equivalent on Windows and in the headless renderer.

### The UI thread owns application state

`AppState` is an `Rc<RefCell<_>>` owned by the Slint event-loop thread. Worker
threads perform blocking I/O and communicate through channels or coalesced
event-loop wakeups. They must not mutate Slint models or `AppState` directly.

This design deliberately avoids an async runtime. Add one only if a concrete
need outweighs the extra runtime and synchronization complexity. Keep expensive
filesystem, Git, subprocess, and terminal reads off the UI thread.

### Files and terminals share one tab model

`WorkspaceTab` owns either a `Document` or a `TerminalSession`. `TabGroups` is a
UI-independent state machine with stable `TabId` values, a maximum of two
groups, and one active tab per group. Each visible group owns its own tab strip
and content surface. A single group must occupy all available workspace space;
an empty secondary group must not reserve layout space.

Docking edge zones create or reposition the secondary group. Body zones move a
tab directly to the primary or secondary group. Changes to this behavior belong
in `src/tabs.rs` first and require state-machine tests before UI gesture tests.

### Document safety takes priority over automatic refresh

Documents are UTF-8 text only and are limited to 2 MiB. `Document` preserves LF
or CRLF endings, owns independent undo/redo history, and tracks a disk revision
using size and modification time.

Saves use a temporary file and backup in the same directory, then replace the
original. Do not replace this with a direct truncate-and-write operation.

The workspace monitor also drives live document refresh:

- A clean open document reloads automatically after an external change.
- A dirty document is never overwritten; the existing Reload/Overwrite conflict
  controls are shown instead.
- If a file is deleted or cannot be read, its in-memory editor buffer remains
  intact and the error is reported.

Any future watcher or collaboration feature must preserve these rules.

### One monitor serves the tree, Git status, and external edits

`StatusMonitor` starts a small Python program. On Windows it runs inside the
selected WSL distribution; on Linux development hosts it runs locally. The
program recursively installs inotify watches, ignores `.git`, `target`,
`node_modules`, and `.cache`, debounces bursts, and runs
`git status --porcelain=v2 -z` for every repository found below the workspace.

Rust receives complete status snapshots through a channel. A 250 ms Slint timer
polls for the newest snapshot, rebuilds the expanded portion of the tree, and
checks open documents for disk revisions. The inotify side currently waits for
roughly 400 ms of quiet time, so external edits normally appear in about
0.4-0.7 seconds.

The monitor must continue to work for non-Git workspaces. Git failures may
remove decorations, but they must not disable editing, terminal use, or file
tree refresh.

### The Windows terminal is a WSL `script` PTY, not ConPTY

The current Windows implementation starts a persistent `wsl.exe` process and
executes `/usr/bin/script -qfec` to allocate the real Linux PTY used by bash and
full-screen terminal applications. Input and output cross Windows pipes. A tty
path written by the shell bootstrap lets a separate background WSL command run
`stty` when the visible terminal size stabilizes.

On non-Windows development builds, `portable-pty` creates a native PTY directly.
Both paths feed a `vt100::Parser` with a 10,000-line scrollback limit.

Terminal output is event-driven. Reader threads send byte chunks and
`OutputSignal` coalesces repeated wakeups into the Slint event loop. The parser
and UI model are then updated on the UI thread. Do not restore a fixed output
polling interval: the previous 33 ms polling loop caused visible typing latency.

Rendering uses sparse cells with explicit row and column coordinates. Default
blank cells are represented by the terminal canvas, while glyphs, cursor cells,
and styled backgrounds remain model entries. The Slint `VecModel` is retained
and changed cells are updated in place, with a bulk reset for large screen
changes. Preserve ANSI backgrounds, wide-character continuation behavior,
cursor position, alternate-screen applications, and Korean IME when optimizing
this path.

Printable terminal text passes through an IME-capable Slint `TextInput`.
Navigation, control, Alt, Enter, Tab, Backspace, and escape keys use explicit VT
encoding. Routing printable ASCII directly around the `TextInput` can break IME,
dead keys, or keyboard layouts and therefore requires dedicated Windows tests.

## Performance Guardrails

- Do not rebuild a dense `rows * columns` Slint terminal component tree for
  every output chunk.
- Coalesce worker notifications before waking the UI event loop.
- Keep syntax highlighting optional for generated, lock, very large, or
  otherwise expensive files. The plain-text editor must remain usable.
- Avoid cloning entire editor or terminal models inside high-frequency input
  callbacks unless the behavioral tests demonstrate that the cost is bounded.
- Measure release builds. Debug rendering and a cold WSL startup are not useful
  product performance baselines.

## Verification Is Part of the Design

The root `AGENTS.md` contains the mandatory policy. In practical terms:

```bash
# Required after production Rust, Slint, build, or runtime-script changes
./scripts/verify

# Also required for UI, Windows integration, dependencies, or release changes
./scripts/verify --windows
```

Compilation alone is not sufficient when behavior can be tested headlessly.

The core Harness uses `#[path = "../../src/..."]` so tests execute production
modules without copying their implementation. Keep reusable policies and state
machines in `src/` rather than embedding them only in `main.rs`.

The UI Harness compiles `../ui/app.slint`, creates a
`MinimalSoftwareWindow`, sends real pointer and keyboard events, and renders
frames for pixel assertions. Update it for layout, focus, selection, clipboard,
IME, tab docking, terminal sizing, and other visible regressions.

When fixing a platform-specific bug, add the nearest portable regression test
and then compile the Windows target. If the behavior depends on actual WSL or
Windows APIs, also document the manual test performed on a real Windows system.

## Getting Started from a Fork

Prerequisites:

- Rust stable with edition 2024 support
- Windows 10 version 1809 or later, or Windows 11
- WSL2 with Ubuntu, bash, Python 3, Git, and util-linux `script`
- Linux development headers listed in `README.md` when running the Slint UI or
  UI Harness under WSL/WSLg

Typical setup:

```bash
git clone <your-fork-url>
cd araseo
./scripts/verify
```

Run a Linux/WSLg development build:

```bash
cargo run -- .
```

Build the native Windows executable from PowerShell:

```powershell
.\scripts\build-windows.ps1
```

That script runs the headless Harness, builds an optimized executable, and
copies it to `dist/araseo.exe`. `dist/` and all Cargo `target/` directories are
generated and ignored by Git; never treat a locally copied executable as the
source of a change.

Install or symlink `scripts/araseo` onto the WSL `PATH`. By default it resolves
the repository's `dist/araseo.exe`; `ARASEO_EXE` can point to another Windows
build:

```bash
export ARASEO_EXE=/mnt/c/Tools/Araseo/araseo.exe
araseo .
```

## Change Checklist

Before opening a pull request:

1. Read the production module and its existing Harness coverage.
2. Keep platform-independent behavior outside Slint callbacks where possible.
3. Add a regression test that fails without the change.
4. Run the verification commands required by `AGENTS.md`.
5. Check `git diff --check` and review only the files intended for the change.
6. For Windows or WSL behavior, build the release executable and perform the
   relevant manual smoke test.

## Current Constraints and Distribution Notes

- The editor intentionally supports a focused subset of IDE behavior; there is
  no LSP, extension host, or workspace-wide indexing service.
- There are at most two visible tab groups and one workspace per process.
- The runtime assumes common Ubuntu paths such as `/bin/bash`, `/usr/bin/script`,
  `/usr/bin/python3`, and `/usr/bin/stty`.
- The package manifest declares the MIT license, but a root `LICENSE` file must
  be added before a public release.
- There is currently no checked-in release workflow or installer. Public
  distribution should build from a version tag, run both Harnesses, package the
  WSL launcher with the Windows executable, publish checksums, and add code
  signing when available.
