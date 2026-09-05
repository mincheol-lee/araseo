# Araseo MVP Product Requirements Document

| Item | Details |
| --- | --- |
| Document status | MVP baseline |
| Product name | Araseo (working title) |
| Target user | Individual developers who use WSL Ubuntu as their primary development environment |
| Target platform | Windows 10 version 1809 or later, or Windows 11, with WSL2 Ubuntu |
| Application type | Native Windows desktop application |
| Primary entry point | `araseo .` from a WSL terminal |

## 1. Product Overview

Araseo is a lightweight personal coding IDE for editing WSL Ubuntu projects and running a shell and Codex in the same window. It does not aim to match the extensibility or language tooling of VS Code. Instead, it delivers three frequently used capabilities quickly and efficiently: file navigation, basic code editing, and an interactive terminal.

The application must use a native Windows window so mouse input, panel resizing, focus movement, and text selection behave like they do in standard Windows applications. Project files, Git, bash, and Codex remain inside the user's WSL Ubuntu environment. The Windows application acts only as a host that displays and controls them.

## 2. Goals and Success Criteria

### 2.1 Goals

- Open the current WSL terminal directory with a single command.
- Provide a file tree, tabbed editor, and real interactive terminal in one window.
- Run `codex` in the terminal using the user's existing WSL configuration.
- Provide editing reliability suitable for routine small code changes.
- Maintain clearly lower resource usage and a shorter startup time than VS Code.

### 2.2 Quantitative Success Criteria

Measurements use an optimized release build, a warm WSL environment rather than the first WSL launch, and a local WSL project containing approximately 1,000 files.

- Display a usable application window within 2 seconds of running the command.
- Use no more than 150 MB of idle memory after opening a project.
- Reflect normal typing on screen within 50 ms.
- Ensure content read from WSL after saving matches the editor content.
- Correctly support Codex alternate-screen mode, colors, cursor movement, keyboard input, and window resizing.

## 3. MVP Scope

### 3.1 Included

- One window and one workspace
- Directory tree navigation and basic file operations
- Unified file and terminal tabs with basic text editing
- Multiple independent WSL bash terminals
- Per-file Git modified and untracked indicators
- An `araseo [PATH]` launcher for WSL terminals
- Clear errors and protection against losing unsaved changes

### 3.2 Excluded

- Syntax highlighting, autocomplete, LSP, go to definition, and diagnostics
- Multiple cursors, minimap, code folding, regular-expression search, and workspace-wide search
- Multiple workspaces and more than two simultaneous tab groups
- A dedicated Codex Agent panel and Codex session management
- Git operation UI such as a diff viewer, staging, committing, and branch switching
- Extensions, remote accounts, and settings synchronization
- Local Windows projects and remote environments other than WSL

## 4. Core User Flows

### 4.1 Opening a Project

1. The user navigates to a project directory in WSL Ubuntu.
2. The user runs `araseo .`.
3. The launcher passes `WSL_DISTRO_NAME` and the normalized absolute Linux path to the Windows executable.
4. Araseo verifies that the path is accessible and opens a Windows window.
5. The project tree appears on the left, an empty editing area appears in the center, and bash starts at the project root in the bottom panel.

If `PATH` is omitted, the launcher uses the current directory. It converts relative paths to absolute Linux paths. If a file path is provided, Araseo opens the file's parent directory as the workspace and opens the file in a tab.

### 4.2 Editing a File

1. The user clicks a file in the tree to open it in a tab.
2. After editing, the tab and status bar indicate that the file has unsaved changes.
3. The user presses `Ctrl+S` to save.
4. After a successful save, the unsaved indicator disappears and Git status refreshes.

### 4.3 Running Codex

1. The user activates an existing terminal tab or creates one with the new-terminal button.
2. The user enters `codex` just as they would in their existing WSL shell.
3. Araseo only relays PTY input/output and the terminal screen. It does not modify the Codex installation, authentication, or configuration.

## 5. Screen and Interaction Design

### 5.1 Default Layout

```text
+------------------+---------------------------------------------+
| File Tree        | Group 1: [file.rs] [>_ terminal] [+]        |
|                  +---------------------------------------------+
|                  |                                             |
|                  | Active file or terminal                     |
|                  |                                             |
|                  +---------------------------------------------+
|                  | Group 2: [README.md] [>_ terminal 2] [+]    |
|                  +---------------------------------------------+
|                  | Active file or terminal                     |
+------------------+---------------------------------------------+
| Workspace | Git | Active file | Cursor/encoding | Status       |
+---------------------------------------------------------------+
```

- Keep the file tree fixed on the left. Every pane owns its own tab bar and active content; there is no global tab strip detached from the panes.
- Support at most two tab groups. Each group may display either a file or terminal, and multiple file and terminal tabs may coexist in either group.
- With one group, its tab bar and active content shall fill the entire workspace area. No space shall be reserved for an empty second group.
- Show a live target preview while a pane-local tab is dragged. Dropping at a workspace edge creates or repositions a split; dropping on the body of the other pane moves the tab into that pane.
- Allow either tab group to fill the work area and restore the previous split layout afterward.
- Place a draggable splitter between the two tab groups in both horizontal and vertical layouts, preserving its ratio during the current application session.
- Give keyboard focus to the clicked panel. Indicate focus through a border or header state.
- Use one dark theme and a system monospace font in the first release.
- Set the minimum window size to 800×600. At smaller sizes, allow the file tree and terminal to collapse.

### 5.2 Default Keyboard Shortcuts

| Action | Shortcut |
| --- | --- |
| Open a file or workspace | `Ctrl+O` |
| Save the current file | `Ctrl+S` |
| Save as | `Ctrl+Shift+S` |
| Find in the current file | `Ctrl+F` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Y` |
| Close the current tab | `Ctrl+W` |
| Toggle terminal visibility | `` Ctrl+` `` |

When the terminal has focus, forward key combinations used by the shell or Codex to the terminal. Initially, intercept only the minimum explicitly defined global UI shortcuts, such as toggling terminal visibility.

## 6. Functional Requirements

### 6.1 Workspace and File Tree

- FR-FS-01: The application shall maintain one WSL distribution and one Linux root path as the workspace.
- FR-FS-02: Directories shall expand using lazy loading, reading children only when needed.
- FR-FS-03: A single click on a file shall open it in a tab. If the file is already open, the application shall activate its existing tab.
- FR-FS-04: The application shall support creating files, creating folders, renaming, and deleting. Before deletion, it shall display the target Linux path and request confirmation.
- FR-FS-05: The `.git` directory shall be hidden by default. Other dotfiles shall remain visible.
- FR-FS-06: Manual refresh shall preserve expanded directories and open tabs whenever possible.
- FR-FS-07: The file tree shall display symbolic links but shall not follow directory links that point outside the workspace.
- FR-FS-08: Permission errors and missing files shall be reported at the affected item without terminating the application.

### 6.2 Editor and Document Lifecycle

- FR-ED-01: Files shall open in tabs, and the same normalized path shall not be opened more than once.
- FR-ED-02: The editor shall support a single cursor, mouse and Shift selection, copying, cutting, and pasting.
- FR-ED-03: Undo and redo history shall be maintained per document and shall remain available after saving until the tab is closed.
- FR-ED-04: The editor shall display line numbers and disable word wrapping by default.
- FR-ED-05: Find shall search for plain strings in the current file and provide next/previous result navigation and a case-sensitivity option.
- FR-ED-06: The Tab key shall insert a literal `\t`, displayed at a default width of four columns.
- FR-ED-07: The editor shall edit UTF-8 text only. If a file cannot be decoded as UTF-8 or contains NUL bytes, the application shall show an unsupported-file message instead of opening it for editing.
- FR-ED-08: Files larger than 2 MiB shall not be read or written. The application shall display the file-size limit.
- FR-ED-09: The editor shall preserve the source file's LF or CRLF line endings. New files shall use LF.
- FR-ED-10: Saving shall write a temporary file in the same directory and replace the original to prevent partial saves. If saving fails, the original file shall remain unchanged.
- FR-ED-11: The application shall retain the file size and modification time recorded when the file was opened. If an external change is detected before saving, the user shall choose among `Reload`, `Overwrite`, and `Cancel`.
- FR-ED-12: Closing an unsaved tab or exiting the application shall offer `Save`, `Don't Save`, and `Cancel`.
- FR-ED-13: If an open file is deleted externally, the application shall retain the tab contents, indicate that the file was deleted, and offer an opportunity to save under another name.

### 6.3 Integrated Terminal

- FR-TR-01: When a workspace opens, the application shall create an initial terminal tab. Each terminal tab shall own an independent ConPTY session and start a login shell with semantics equivalent to the following command:

  ```text
  wsl.exe -d <distro> --cd <linux-workspace-root> bash -l
  ```

- FR-TR-02: The terminal shall support 256 colors and true color, bold/italic/underline attributes, Unicode, cursor rendering, and the alternate screen.
- FR-TR-03: The application shall calculate rows and columns from the terminal's pixel dimensions and resize both ConPTY and the screen model when the splitter or window size changes.
- FR-TR-04: The application shall forward keyboard input, control keys, and pasted text as UTF-8 and appropriate VT input sequences.
- FR-TR-05: The terminal shall retain up to 10,000 lines of scrollback and remove the oldest lines when the limit is exceeded.
- FR-TR-06: Dragging shall perform local text selection by default. When the running application requests mouse reporting, ordinary clicks and wheel events shall be forwarded while `Shift+drag` remains available for local selection.
- FR-TR-07: If the shell exits, the terminal shall display its exit code and provide a `Restart` action.
- FR-TR-08: On application exit, Araseo shall close PTY input and wait for the child process to terminate, then clean up any remaining host process after a timeout.
- FR-TR-09: The user shall be able to create and close multiple terminal tabs. Closing one terminal tab shall terminate only its PTY session.
- FR-TR-10: A terminal tab shall show a numbered, shortened starting-directory label in its pane-local tab bar and the full starting path in the window title/status context.

### 6.4 Git Status

- FR-GIT-01: The application shall run `git` from the workspace's WSL distribution rather than Windows Git.
- FR-GIT-02: The application shall parse `git status --porcelain=v2 -z` and display modified and untracked states in the file tree.
- FR-GIT-03: Git status shall refresh when the project opens, after saving a file, when focus returns to the UI from the terminal, and on manual refresh.
- FR-GIT-04: Editing and terminal functionality shall continue to work when the workspace is not a Git repository or Git is not installed.
- FR-GIT-05: The UI shall not run repository-mutating Git commands such as stage, commit, or checkout.

## 7. Technical Design

### 7.1 Technology Stack

| Area | Choice | Rationale |
| --- | --- | --- |
| Language | Rust stable | A single native executable, explicit resource ownership, and low runtime overhead |
| GUI | Slint | Direct Rust integration without requiring an Electron or WebView runtime |
| Windowing/rendering | Slint `winit` + FemtoVG | A native Windows window and GPU rendering with a smaller dependency footprint than Skia |
| Editing control | Wrapper based on Slint `TextEdit` | Meets the MVP editing scope without implementing an entire code editor from scratch |
| PTY | `portable-pty` | Abstracts Windows ConPTY creation and the process I/O lifecycle |
| Terminal model | `vt100` | Converts ANSI/VT bytes into a screen cell grid, colors, cursor, and modes |
| Asynchronous work | Rust threads/channels | Separates file, PTY, and Git I/O from the UI event loop while limiting runtime size |

Slint's `TextEdit` provides multiline input, selection, clipboard integration, and keyboard events, but it does not provide code-editor features. For the MVP, Araseo wraps it with a document model that implements find, save state, and limited undo/redo. If syntax highlighting or LSP support becomes necessary, the editor layer will be evaluated separately.

References:

- [Slint TextEdit](https://docs.slint.dev/latest/docs/slint/reference/std-widgets/views/textedit/)
- [Slint backends and renderers](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/)
- [Microsoft ConPTY](https://learn.microsoft.com/en-us/windows/console/pseudoconsoles)
- [vt100 crate](https://docs.rs/vt100/latest/vt100/)

### 7.2 Process Boundaries

```text
WSL Ubuntu                                  Windows
┌─────────────────────┐                    ┌──────────────────────────┐
│ araseo shell wrapper│ --distro/root-->  │ Araseo.exe + Slint UI    │
│                     │                    │                          │
│ project files       │ <----- UNC -----> │ filesystem/document core │
│ git                 │ <--- wsl.exe ---- │ Git status worker        │
│ bash / Codex        │ <--- ConPTY ----> │ terminal worker + vt100  │
└─────────────────────┘                    └──────────────────────────┘
```

The Windows application maps an absolute Linux path to a UNC path using the following rule:

```text
/home/minch/project
→ \\wsl.localhost\Ubuntu\home\minch\project
```

The UI thread is responsible only for updating Slint models and handling input. File opening and saving, Git queries, and PTY reads run in workers that send results to the UI thread through channels. The editor and panel interactions must remain responsive during long-running operations.

### 7.3 Public CLI Contract

User-facing interface:

```text
araseo [PATH]
araseo --help
araseo --version
```

Internal interface passed to the Windows executable:

```text
Araseo.exe --distro <WSL_DISTRO_NAME> --root <LINUX_ABSOLUTE_PATH> [--file <LINUX_ABSOLUTE_PATH>]
```

- The `araseo` launcher is a WSL-only POSIX shell script.
- `PATH` defaults to the current directory.
- Distribution names and paths shall be passed as individual arguments without concatenating them into a shell command and evaluating them again.
- Reusing an existing window for the same workspace is not guaranteed in the MVP. Each invocation may open a new window.

### 7.4 Core Domain Types

```rust
struct Workspace {
    distro: String,
    linux_root: PathBuf,
    unc_root: PathBuf,
}

struct Document {
    linux_path: PathBuf,
    text: String,
    line_ending: LineEnding,
    dirty: bool,
    disk_revision: DiskRevision,
}

struct FileNode {
    name: String,
    linux_path: PathBuf,
    kind: FileKind,
    expanded: bool,
    git_status: GitStatus,
}

enum GitStatus { Clean, Modified, Untracked }

struct TerminalSession {
    rows: u16,
    columns: u16,
    running: bool,
    exit_code: Option<i32>,
}

struct TerminalCell {
    text: String,
    foreground: TerminalColor,
    background: TerminalColor,
    attributes: CellAttributes,
}
```

The UI consumes read-only view models of this state and sends explicit command callbacks such as `open_file`, `save_document`, `resize_terminal`, and `write_terminal_input` to the Rust core. UI files shall not manipulate files or processes directly.

## 8. Error Handling and Safety

| Situation | Behavior |
| --- | --- |
| WSL or the specified distribution is unavailable | Display the cause and a command the user can run to diagnose it; do not open the workspace |
| UNC path access fails | Display the Linux path and distribution and provide a retry action |
| File cannot be read or saved due to permissions | Display the error in the affected tab and preserve the current edited content |
| File changed externally | Offer Reload, Overwrite, and Cancel |
| Git is unavailable or the workspace is not a repository | Disable only Git indicators and keep all other functionality available |
| `codex` is unavailable | Display the shell's normal command-not-found output |
| PTY or bash exits | Display the exit code and a restart button |
| Terminal output floods the UI | Batch UI updates and enforce the scrollback limit |

- All file operations shall accept only normalized paths inside the workspace. The application shall prevent symbolic links from escaping the workspace.
- Paths shall be passed through OS argument arrays rather than interpolated into shell command strings, safely supporting spaces, Korean characters, and quotation marks.
- The application shall not separately read or store Codex authentication tokens or shell environment variables.
- Because deletion may be irreversible, the application shall always request explicit confirmation. For non-empty directories, it shall also warn with the number of affected items.

## 9. Verification Plan

### 9.1 Automated Tests

- Conversion between Linux and UNC paths, including spaces, Korean characters, dotfiles, and long paths
- Workspace boundary validation and prevention of `..` and symbolic-link escapes
- UTF-8 validation, NUL detection, the 2 MiB limit, and LF/CRLF preservation
- Document dirty state, undo/redo, saved revisions, and external-change detection
- Parsing modified and untracked entries from `git status --porcelain=v2 -z`
- Conversion of keyboard events into VT input sequences
- VT color, cursor, alternate-screen, and resize state handling
- Enforcement of the 10,000-line terminal scrollback limit

### 9.2 Windows–WSL Integration Tests

- Launch with `araseo .`, a relative path, an absolute path, and a single file
- Create, rename, edit, atomically save, and delete files, then verify the results from WSL
- Modify an open file externally from WSL and verify every conflict-resolution option
- Verify behavior in a Git repository, a non-repository directory, and an environment without Git installed
- Run `codex` in bash and verify input, colors, alternate screen, paste, scroll, and resize behavior
- Verify normal shell exit, abnormal shell exit, and terminal restart
- Verify handling of unsaved documents and cleanup of a running PTY when the application exits

### 9.3 Performance Tests

- Startup time and initial tree-render time for a project containing 1,000 files
- Memory usage with an empty workspace, ten open tabs, and Codex running
- UI input latency during rapid typing and high-volume terminal output
- UI responsiveness during lazy loading of large directories and Git status refreshes

## 10. Release Phases

### MVP

Complete the file tree, basic editor, unified file/terminal tabs, Git status indicators, and `araseo .` launch flow defined in this document.

### Phase 2 Candidates

- Tree-sitter-based syntax highlighting
- Persist tab groups and split ratios between application sessions
- Read-only Git diff viewer
- Persistence for recent workspaces and basic UI settings

### Phase 3 Candidates

- LSP autocomplete, diagnostics, and go to definition
- Codex Agent panel, session status, and approval UI
- Git staging and commit UI

If the Slint editor does not meet acceptance criteria for real-world usability, IME, large documents, or rendering performance, a migration to Monaco/Tauri shall be evaluated as a separate technical decision. The MVP shall not maintain both UI stacks simultaneously.

## 11. MVP Definition of Done

The MVP is complete when all of the following conditions are met:

- Running `araseo .` from WSL Ubuntu opens a native Windows window.
- The user can navigate a project in the file tree and safely edit and save multiple UTF-8 files.
- Unsaved changes and external file modifications never cause edited content to be lost without explicit user action.
- Bash and Codex can be used interactively in the terminal within the same window.
- Git modified and untracked states appear in the file tree.
- Errors are presented as understandable application states rather than causing the entire application to crash.
- Quantitative performance goals and automated and integration tests pass.
