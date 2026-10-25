# Project code navigation

Araseo uses local language-server processes in the selected WSL distribution.
It does not send source code to a cloud service. Servers start on demand, keep
an analysis cache between requests, and are separate from the UI and PTYs.

## Using navigation

| Action | Mouse | Windows / Linux shortcut |
| --- | --- | --- |
| Search project functions/methods | **Functions** in the status bar, or **Search Project Functions…** on a folder | `Ctrl+T` outside terminals |
| Go to definition | `Ctrl+click`, or editor context menu | `F12` |
| List definitions in sidebar | Editor context menu | `Alt+F12` |
| Find all references | Editor context menu | `Shift+F12` |
| Show incoming callers | Editor context menu | `Shift+Alt+H` |
| Back / forward | Status bar arrows, or editor context menu | `Alt+Left` / `Alt+Right` outside terminals |

Place the editor cursor on a symbol before requesting references or callers.
All project search, reference and caller results use the **left sidebar**.
The function search shows its **Project** path and selected language. Select a
project or language selector to change the search. Use Up/Down and Enter to open
results in the editor; the results remain visible while you read or edit code.
Escape or the close button closes navigation. Files/Git switch sidebar views;
the navigation button restores the same results. Drag the sidebar edge to resize.

**Find All References** includes declarations and groups occurrences by file.
Expand/collapse file groups, click a file heading or occurrence to open it.
Occurrences show a bounded source-line preview with the referenced text highlighted;
open unsaved buffers take precedence over disk content. The summary counts results
and files. Source previews are read on workers and have a 16 MiB total disk budget.

**Call Hierarchy** retains the target function as the tree root and initially
expands its direct incoming callers. Click a caller to open its first call site,
or use the disclosure button (or Right arrow) to follow that caller's own incoming
calls. Left collapses an expanded branch. A caller with several call sites is one
tree node; Find All References lists its individual occurrences. Refresh restarts
the original query; expansions and searches synchronize the latest open buffers.
If the server does not implement
call hierarchy, the UI explicitly labels its fallback as **references**: these
can include passing a function as a value, not only calling it.

The current file determines the default project: Araseo finds the nearest
repository or language-project marker, such as `Cargo.toml`, `package.json`,
`pyproject.toml`, `go.mod`, or `compile_commands.json`. Starting the window in
`/home/user` does not merge all neighboring projects into a single search.
For an unmarked folder, the first directory below the window workspace is used.
A folder context menu chooses the project containing that folder. Select a
repository/root in the project selector to search its full project rather than a nested package.

Definition results may point into dependencies outside the project. Araseo can
open these local WSL files, while function search, references, and callers stay
inside the selected project. Existing file tabs and their split panes are
reused. Back/forward records the file, selection and scroll position, and returns
to the original tab if it still exists. This history lasts for the current app
session. It does not close or navigate terminal tabs.

## Install language servers in WSL

Servers are optional and are not bundled into the Windows executable. Install
only those you need. A missing server produces an error in the sidebar (or status
bar for definition requests). **Copy WSL install command** copies the command
for the selected language. Run it in a WSL terminal, then retry. No administrator
privileges are needed for the commands below.

| Language | Server | WSL installation |
| --- | --- | --- |
| Rust | rust-analyzer | `rustup component add rust-analyzer` |
| Python | Pyright | `npm install --prefix ~/.local/share/araseo/lsp pyright` |
| JavaScript / TypeScript / JSX / TSX | typescript-language-server | `npm install --prefix ~/.local/share/araseo/lsp typescript@6 typescript-language-server` |
| Go | gopls | `mkdir -p ~/.local/bin; GOBIN=$HOME/.local/bin go install golang.org/x/tools/gopls@latest` |
| C / C++ | clangd | Use an existing WSL clangd installation or extract a compatible release into a user directory and configure its absolute path. A compilation database improves results. |

Rust needs a Rust toolchain; Python/TypeScript servers need Node.js/npm in WSL;
Go needs Go. Araseo launches through the WSL login shell and also adds
`~/.cargo/bin`, `~/.local/bin`, `~/go/bin`, and
`~/.local/share/araseo/lsp/node_modules/.bin` to the server PATH. If Node.js is
absent in that shell and nvm is installed, its configured default is loaded.
Python 3 is used
for pipe supervision, as it is for the existing WSL Git monitor.

The TypeScript adapter currently uses the JavaScript `tsserver` API. TypeScript
6 is the tested compatible installation; the TypeScript 7 native package alone
does not contain the tsserver required by this adapter.

Rust, Python and TypeScript definition, function search and incoming callers
have been checked against real servers. Go and C/C++ use the same protocol
adapter; accuracy and available features depend on the installed server and
project configuration.

## Optional configuration

`code-navigation.json` sits next to `fonts.conf`:

- Windows: `%LOCALAPPDATA%\araseo\code-navigation.json`
- Linux: `$XDG_CONFIG_HOME/araseo/code-navigation.json`, or
  `~/.config/araseo/code-navigation.json`

Configuration is global to the user, not loaded from untrusted project files.
Restart Araseo after changing the configuration. Omitted values keep defaults;
`args` replaces the default argument list, while `options` and `settings` merge
with defaults. An empty `command` disables that language.

```json
{
  "Rust": { "command": "/home/me/.cargo/bin/rust-analyzer" },
  "Cpp": {
    "command": "/home/me/tools/clangd/bin/clangd",
    "args": ["--background-index", "-j=2", "--limit-results=500"]
  },
  "Python": {
    "settings": {
      "python": { "pythonPath": "/home/me/project/.venv/bin/python" }
    }
  }
}
```

Other language keys are `TypeScript` and `Go`. Commands and arguments are passed
as quoted executable arguments; they are not executable shell snippets.

## Responsiveness and limits

- Analysis, filesystem project discovery and process I/O run on workers.
- Search is debounced; a newer request cancels/discards obsolete results.
- Open, unsaved buffers are synchronized before each navigation request.
  Incremental servers receive a valid full-buffer replacement range.
- Locations use LSP UTF-16 units, including Korean and supplementary characters.
- The UI renders visible result rows only; results are capped at 500 and caller
  expansion at 12 levels. Narrow the search if it reaches the result limit.
- At most three project/language servers stay cached. Unused servers expire
  after 15 minutes. Requests have deadlines; failed/hung processes can restart
  on retry. The WSL supervisor terminates the server group when its input closes.
- Rust automatic Cargo checks, build-script execution, procedural macros and
  eager cache warming are disabled by default; two analysis threads are used.
  Function search includes all symbol kinds and is filtered to functions/methods.
  Rust-analyzer watches files itself. Macros/build-generated code may need
  explicitly enabling its build-script/proc-macro options for accurate results.

Language-server indexing consumes CPU and memory, especially on first use.
These controls keep it off the UI path; they do not make large-project analysis
free. The first request can require a retry while a server indexes the project.
