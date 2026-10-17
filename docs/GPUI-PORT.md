# GPUI Kit frontend migration

Branch: `feat/gpui-kit-port`, based on main `a5448b1` (0.1.22).

## Architecture

The production application uses GPUI Kit 0.7.3 and its pinned upstream GPUI
snapshot, with the native DirectX backend on Windows. There is one frontend;
Slint dependencies, generated UI, renderer patches and fixtures are removed.

- `gpui_app.rs`: workbench, lazy file tree, Git sidebar, dialogs, appearance,
  diagnostics/process control, background jobs and session persistence.
- `gpui_chrome.rs`: one shared row for upper-right dock tabs, terminal creation
  and native window controls; bundled sidebar action icons and application logo.
- `git_view.rs`: deterministic repository groups, relative folder labels and
  collapsed rows for the virtualized Git sidebar, without filesystem access.
- `gpui_panel.rs`: persistent dockable file/preview/diff/terminal entities.
- `gpui_editor.rs`: Kit editor plus Araseo's independent font brightness.
- `gpui_terminal.rs`: native input/IME client and one painted terminal surface.
- `terminal_paint.rs`: adjacent style runs and UTF-16 composition helpers.

The PTY transport, resize pipe, lossless VT worker, bounded input queue, protocol
query replies, scrollback, document conflict checks and filesystem/Git modules
remain the production backend. A running terminal is not recreated when docked.
Only terminal entities are invalidated by output. Unchanged rows reuse both
snapshot ownership and shaped runs. Hidden terminals continue parsing while
presentation pauses.

## Functional coverage

- Lazy file browsing and Git folder colors; create, rename, delete, move/copy,
  Linux-path copy, external file drops and additional workspace folders.
- Source editing, syntax highlighting, search, selection/clipboard, undo/redo,
  asynchronous save/reload, external-change protection and unsaved recovery.
- Markdown source/preview, full-resolution raster/SVG images, image/PDF fit and
  zoom, PDF pages and text selection/copy; native side-by-side Git diff.
- Live terminals, independent font settings, agent activity markers, terminal
  selection/copy/paste/scrolling, native IME preedit/commit and path drops.
- Draggable/reorderable tabs, nested resizable splits, pane zoom and restored
  layout; sidebar resize, window geometry and added folders.
- WSL diagnostics, Docker usage, guarded process stop/kill and refreshed results.
- Existing panic/Windows-exception logging, module snapshots, focus changes,
  long event-loop pauses and periodic heartbeats.

The layout stores Kit's serializable dock state in an optional session field.
Old session files still load through their original split tree. Unsaved text
and original disk revisions retain the existing conflict behavior. Background
session writes have monotonically ordered generations, so an older autosave
cannot overwrite the close snapshot. A normal close waits for the final local
save. Terminal restoration starts new shells; running commands do not resume.
Agent activity uses the existing batched process probe and hook state. Claude
and Codex terminal tabs display their bundled SVG logos. A retained icon view
rotates working agents once per 800 ms at at most 10 redraws per second; waiting
agents remain still. Rotation stops when the window is inactive or reduced
motion is enabled. No repeating application timer is added: the icon schedules
its next frame only while rendered and working, allowing cached terminal/editor
content to be reused.

The compact workbench removes the separate workspace-path header. The logo
aligns with the top dock tabs, and file/folder/project creation icons sit beside
FILES. New entries use the selected tree directory (or the selected file's
parent), falling back to the primary workspace. Project addition retains the
WSL path dialog and native folder browser. Caption controls share the upper-right
tab row, following nested splits and pane zoom without overlaying the tabs.

The existing application logo is also packaged as `assets/araseo.ico`, with
16/24/32/48/64/128/256px images. `build.rs` embeds it as Windows icon resource 1
in the application binary, matching GPUI's native window-class/taskbar lookup.
The Windows workbench check rejects a missing native class icon and captures
the actual icon as `window-icon.png`.

Git changes are grouped by their deepest owning repository, including projects
below a shared starting folder and explicitly added workspaces. Each group has
its project path, change count and collapse control; file rows retain the
filename, relative parent folder and status badge. The grouped model is rebuilt
on status updates rather than on terminal output or every workbench render.
The sidebar creates only visible rows. Existing diff-opening and discard actions
remain attached to each file's full path.

Diff columns follow the current dock panel width, with fixed HEAD/Working Tree
headings and soft wrapping. The longest source line no longer determines the
combined width or pushes the modified side outside the visible window. The
production component retains source-based selection, copy and vertical scrolling.
The panel observes the retained diff state so the initial viewport, background
syntax results and selection changes trigger a repaint. A measured panel-size
change schedules one redraw after paint, allowing the Kit to use its updated
viewport when a cached dock panel shrinks or grows; unchanged bounds do not
schedule additional frames.

## Build support

`vendor/gpui-pre-windows` is the published 0.3.9 crate with a build-script-only
change: compile release HLSL shaders for the Windows **target** even when Cargo
runs on Linux/WSL, invoking the host Windows SDK compiler with Windows paths.
Shader/runtime sources are unchanged. Preserve the included Apache license.
Set `GPUI_FXC_PATH` if the SDK is not in its standard location.

Normal verification runs core tests, production GPUI UI tests, and installer
fixtures. Windows verification additionally compiles the app and core tests for
Windows GNU. Native PowerShell checks drive the actual release executable;
there are no separate copies of the production UI in test fixtures.

## Validation limits

The Windows GNU release was exercised on the Windows host, including native
continuous input after dragging a running terminal into a lower split, session
restoration, shrink/grow, maximize/restore and minimize/restore. The resize
checks compare the displayed surface before and after an explicit repaint.
Mixed ASCII/Korean terminal output was also visually inspected. The headless
suite passes 120 core tests and 14 production GPUI UI tests, plus installer and
launcher fixtures. The executable retains its matching embedded GNU debug
information.

Terminal input regressions cover Tab/Backtab remaining inside the terminal and
positive upward line/pixel wheel deltas opening older history. The native
`TerminalInput` check records actual key and SGR wheel reports in both normal
and alternate screen buffers, without launching agent clients.

The compact-header follow-up additionally exercises actual file/folder/project
creation through the Windows icon buttons, and clicks the custom caption buttons
to minimize, maximize, restore and close. Headless checks cover the 150px sidebar,
new-file opening, terminal creation after a dialog, and caption placement with a
right split and pane zoom. Native test typing confirms dialogs with a queued
Enter, so UI Automation cannot invoke confirmation before queued text is received.

A successful build or a GPU backend alone does not establish a speedup or
crash-rate improvement. Repeat the same Claude/Codex workloads on target
hardware before comparing latency, CPU, memory or long-session reliability.
Headless IME tests cover event handling and UTF-16 behavior; they cannot certify
every physical Windows IME or laptop-specific composition configuration.

The Git/diff follow-up adds headless tests with two real repositories, grouped
counts and collapse/expand, then opens a change through the production sidebar.
A production diff panel is resized from 1000px to 420px to 760px; both columns
remain within their halves, text reflows, and copying still returns the exact
unwrapped Unicode source. A second check resizes the complete workbench's docked
diff through 1250px, 850px and 1100px, without forcing a refresh.
`test-windows-workbench.ps1 -Mode Git` exercises the
same grouping and resize behavior through the Windows executable and accessibility
bounds, using isolated temporary repositories and settings.
The final Windows executable passed the native 1180px -> 820px -> 1180px resize
sequence with both source sides visible before any explicit repaint. Screenshots
were also inspected for the grouped sidebar and narrow wrapped diff.

The agent-tab follow-up verifies Claude/Codex logo selection in the production
DockSkin, removal after exit, 10 Hz animation notifications, reuse of cached
content, waiting-state stop, reduced motion and blur/reactivation. The native
`test-windows-workbench.ps1 -Mode Agent` fixture uses terminal-local process
names and the existing activity-file protocol, without contacting either agent
service; its icon screenshots must change while working and remain identical
while waiting.
The final Windows fixture passed for both agent kinds: working captures had
changing icon pixels, waiting captures were identical, and the icon disappeared
after each terminal-local process exited. The screenshots retained the existing
orange Claude and white Codex logos.
