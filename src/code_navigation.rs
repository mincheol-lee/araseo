//! Project-scoped semantic navigation, independent of GPUI and terminal input.
use crate::{
    lsp_transport::{Cancellation, Client, Result},
    workspace::Workspace,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, TryLockError},
    time::{Duration, Instant},
};

pub const LIMIT: usize = 500;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    Go,
    Cpp,
}
impl Language {
    pub const ALL: [Self; 5] = [
        Self::Rust,
        Self::Python,
        Self::TypeScript,
        Self::Go,
        Self::Cpp,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::TypeScript => "TypeScript / JavaScript",
            Self::Go => "Go",
            Self::Cpp => "C / C++",
        }
    }
    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "rs" => Some(Self::Rust),
            "py" | "pyi" => Some(Self::Python),
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(Self::TypeScript),
            "go" => Some(Self::Go),
            "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" => Some(Self::Cpp),
            _ => None,
        }
    }
    pub fn language_id(self, path: &Path) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Go => "go",
            Self::Cpp => {
                if path.extension().is_some_and(|e| e == "c") {
                    "c"
                } else {
                    "cpp"
                }
            }
            Self::TypeScript => match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "tsx" => "typescriptreact",
                "jsx" => "javascriptreact",
                "js" | "mjs" | "cjs" => "javascript",
                _ => "typescript",
            },
        }
    }
    pub fn install(self) -> &'static str {
        match self {
            Self::Rust => "rustup component add rust-analyzer",
            Self::Python => "npm install --prefix ~/.local/share/araseo/lsp pyright",
            Self::TypeScript => {
                "npm install --prefix ~/.local/share/araseo/lsp typescript@6 typescript-language-server"
            }
            Self::Go => "GOBIN=$HOME/.local/bin go install golang.org/x/tools/gopls@latest",
            Self::Cpp => {
                "Install clangd in WSL and add its directory to PATH (or configure an absolute command in code-navigation.json)."
            }
        }
    }
    fn spec(self) -> ServerSpec {
        let (command, args, options, settings) = match self {
            Self::Rust => (
                "rust-analyzer",
                vec![],
                json!({"checkOnSave":false,"cargo":{"buildScripts":{"enable":false}},"procMacro":{"enable":false},"cachePriming":{"enable":false},"numThreads":2,"files":{"watcher":"server"},"workspace":{"symbol":{"search":{"kind":"all_symbols","scope":"workspace","limit":500}}}}),
                json!({"rust-analyzer":{"checkOnSave":false,"cargo":{"buildScripts":{"enable":false}},"procMacro":{"enable":false},"cachePriming":{"enable":false},"numThreads":2,"files":{"watcher":"server"},"workspace":{"symbol":{"search":{"kind":"all_symbols","scope":"workspace","limit":500}}}}}),
            ),
            Self::Python => (
                "pyright-langserver",
                vec!["--stdio"],
                json!({}),
                json!({"python":{"analysis":{"diagnosticMode":"openFilesOnly","typeCheckingMode":"off","autoSearchPaths":true}}}),
            ),
            Self::TypeScript => (
                "typescript-language-server",
                vec!["--stdio"],
                json!({"maxTsServerMemory":1024}),
                json!({}),
            ),
            Self::Go => (
                "gopls",
                vec!["serve"],
                json!({"staticcheck":false}),
                json!({"gopls":{"staticcheck":false}}),
            ),
            Self::Cpp => (
                "clangd",
                vec!["--background-index", "-j=2", "--limit-results=500"],
                json!({}),
                json!({}),
            ),
        };
        ServerSpec {
            command: command.into(),
            args: args.into_iter().map(str::to_owned).collect(),
            options,
            settings,
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(default)]
struct ServerSpec {
    command: String,
    args: Vec<String>,
    options: Value,
    settings: Value,
}
impl Default for ServerSpec {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: vec![],
            options: json!({}),
            settings: json!({}),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}
#[derive(Clone, Debug)]
pub struct Snippet {
    pub text: String,
    pub highlight: std::ops::Range<usize>,
}
#[derive(Clone, Debug)]
pub struct Row {
    pub name: String,
    pub snippet: Option<Snippet>,
    pub detail: String,
    pub location: Location,
    pub item: Option<Value>,
    pub depth: usize,
}
#[derive(Clone)]
pub struct Snapshot {
    pub path: PathBuf,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct Project {
    pub workspace: Workspace,
    pub root: PathBuf,
}
impl Project {
    pub fn label(&self) -> String {
        self.root.display().to_string()
    }
}
#[derive(Clone)]
pub enum Query {
    Symbols(String),
    Definition(Position, PathBuf),
    References(Position, PathBuf),
    Callers(Position, PathBuf),
    Incoming(Value, usize),
}
#[derive(Clone)]
pub struct Request {
    pub project: Project,
    pub language: Language,
    pub query: Query,
    pub snapshots: Vec<Snapshot>,
}
#[derive(Debug)]
pub struct Response {
    pub rows: Vec<Row>,
    pub note: String,
}

/// Find the nearest language project or repository. Never index the whole home
/// directory merely because it is the window's starting workspace.
pub fn project_for(workspace: &Workspace, path: &Path, directory: bool) -> Result<Project> {
    if !path.starts_with(&workspace.linux_root) {
        return Err("Project is outside the workspace".into());
    }
    let start = if directory {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let markers = [
        ".git",
        "Cargo.toml",
        "package.json",
        "tsconfig.json",
        "jsconfig.json",
        "pyproject.toml",
        "pyrightconfig.json",
        "setup.py",
        "go.mod",
        "compile_commands.json",
        "CMakeLists.txt",
    ];
    let mut root = None;
    for candidate in start
        .ancestors()
        .take_while(|p| p.starts_with(&workspace.linux_root))
    {
        let host = workspace
            .host_root
            .join(candidate.strip_prefix(&workspace.linux_root).unwrap());
        if markers.iter().any(|marker| host.join(marker).exists()) {
            root = Some(candidate.to_path_buf());
            break;
        }
    }
    let root = root.unwrap_or_else(|| {
        let relative = start
            .strip_prefix(&workspace.linux_root)
            .unwrap_or(Path::new(""));
        relative
            .components()
            .next()
            .map(|part| workspace.linux_root.join(part))
            .unwrap_or_else(|| start.to_path_buf())
    });
    let root_text = root.to_string_lossy().replace('\\', "/");
    if root_text == "/"
        || root_text == "/home"
        || root_text == "/root"
        || (root_text.starts_with("/home/") && root.components().count() <= 3)
    {
        return Err("Choose a project folder before starting code navigation; the home directory is not a project".into());
    }
    Ok(Project {
        workspace: workspace.clone(),
        root,
    })
}

pub fn uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
pub fn uri_path(uri: &str) -> Result<PathBuf> {
    let path = uri
        .strip_prefix("file://")
        .ok_or("Only local file locations are supported")?;
    if !path.starts_with('/') {
        return Err("Language server returned a non-local file authority".into());
    }
    let mut bytes = Vec::new();
    let input = path.as_bytes();
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            if i + 2 >= input.len() {
                return Err("Invalid file URI".into());
            }
            bytes
                .push(u8::from_str_radix(&path[i + 1..i + 3], 16).map_err(|_| "Invalid file URI")?);
            i += 3;
        } else {
            bytes.push(input[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    if text.contains('\0') {
        return Err("Invalid file URI".into());
    }
    let path = PathBuf::from(text);
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Invalid file URI traversal".into());
    }
    Ok(path)
}
pub fn position(text: &str, byte: usize) -> Position {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    let prefix = &text[..byte];
    Position {
        line: prefix.bytes().filter(|b| *b == b'\n').count() as u32,
        character: prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count() as u32,
    }
}
pub fn offset(text: &str, pos: Position) -> usize {
    let mut line_start = 0;
    for _ in 0..pos.line {
        match text[line_start..].find('\n') {
            Some(i) => line_start += i + 1,
            None => return text.len(),
        }
    }
    let line = text[line_start..].split('\n').next().unwrap_or("");
    let mut units = 0;
    for (index, ch) in line.char_indices() {
        if units + ch.len_utf16() > pos.character as usize {
            return line_start + index;
        }
        units += ch.len_utf16();
    }
    line_start + line.trim_end_matches('\r').len()
}
fn location(value: &Value) -> Option<Location> {
    let uri = value["uri"]
        .as_str()
        .or_else(|| value["targetUri"].as_str())?;
    let range = value
        .get("targetSelectionRange")
        .or_else(|| value.get("selectionRange"))
        .or_else(|| value.get("range"))?;
    Some(Location {
        path: uri_path(uri).ok()?,
        range: serde_json::from_value(range.clone()).ok()?,
    })
}
fn function(kind: u64) -> bool {
    matches!(kind, 6 | 9 | 12)
}
fn rows(value: Value, root: Option<&Path>, functions: bool) -> Vec<Row> {
    let values = if let Value::Array(array) = value {
        array
    } else if value.is_object() {
        vec![value]
    } else {
        vec![]
    };
    let mut rows = vec![];
    for value in values {
        if functions && !value["kind"].as_u64().is_some_and(function) {
            continue;
        }
        let loc = location(value.get("location").unwrap_or(&value));
        let Some(loc) = loc else {
            continue;
        };
        if root.is_some_and(|root| !loc.path.starts_with(root)) {
            continue;
        }
        rows.push(Row {
            snippet: None,
            name: value["name"].as_str().unwrap_or("Location").into(),
            detail: value["containerName"].as_str().unwrap_or("").into(),
            location: loc,
            item: None,
            depth: 0,
        });
        if rows.len() == LIMIT {
            break;
        }
    }
    rows.sort_by(|a, b| {
        (
            &a.location.path,
            a.location.range.start.line,
            a.location.range.start.character,
            &a.name,
        )
            .cmp(&(
                &b.location.path,
                b.location.range.start.line,
                b.location.range.start.character,
                &b.name,
            ))
    });
    rows.dedup_by(|a, b| a.location == b.location && a.name == b.name);
    rows
}

struct Server {
    root: PathBuf,
    distro: String,
    language: Language,
    client: Client,
    documents: HashMap<PathBuf, (String, i32)>,
    last_used: Instant,
    started: Instant,
}
#[derive(Default)]
struct State {
    servers: Vec<Server>,
}
#[derive(Clone, Default)]
pub struct Service {
    state: Arc<Mutex<State>>,
    config: Option<PathBuf>,
}
impl Service {
    pub fn new(config: Option<PathBuf>) -> Self {
        let service = Self {
            config,
            ..Self::default()
        };
        let weak = Arc::downgrade(&service.state);
        let _ = std::thread::Builder::new()
            .name("araseo-lsp-idle".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(5));
                    let Some(state) = weak.upgrade() else {
                        break;
                    };
                    if let Ok(mut state) = state.try_lock() {
                        state.servers.retain(|s| {
                            s.client.is_alive()
                                && s.last_used.elapsed() < Duration::from_secs(15 * 60)
                        });
                    };
                }
            });
        service
    }
    pub fn execute(&self, request: Request, cancel: &Cancellation) -> Result<Response> {
        // Contention is cancellable; only workers ever enter this code.
        let mut state = loop {
            cancel.check()?;
            match self.state.try_lock() {
                Ok(state) => break state,
                Err(TryLockError::Poisoned(e)) => break e.into_inner(),
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(15)),
            }
        };
        state.servers.retain(|s| {
            s.client.is_alive() && s.last_used.elapsed() < Duration::from_secs(15 * 60)
        });
        let index = state.servers.iter().position(|s| {
            s.root == request.project.root
                && s.distro == request.project.workspace.distro
                && s.language == request.language
        });
        let index = if let Some(index) = index {
            index
        } else {
            if state.servers.len() >= 3 {
                let oldest = state
                    .servers
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, s)| s.last_used)
                    .unwrap()
                    .0;
                state.servers.remove(oldest);
            }
            let mut spec = request.language.spec();
            if let Some(path) = &self.config {
                if path.exists() {
                    let bytes =
                        std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                    let config: HashMap<Language, Value> = serde_json::from_slice(&bytes)
                        .map_err(|e| format!("Invalid code-navigation.json: {e}"))?;
                    if let Some(custom) = config.get(&request.language) {
                        if let Some(command) = custom.get("command") {
                            spec.command = command
                                .as_str()
                                .ok_or("Server command must be a string")?
                                .into();
                        }
                        if let Some(args) = custom.get("args") {
                            spec.args = serde_json::from_value(args.clone())
                                .map_err(|e| format!("Invalid server arguments: {e}"))?;
                        }
                        if let Some(options) = custom.get("options") {
                            merge_json(&mut spec.options, options);
                        }
                        if let Some(settings) = custom.get("settings") {
                            merge_json(&mut spec.settings, settings);
                        }
                    }
                }
            }
            if spec.command.is_empty() {
                return Err(format!(
                    "{} server is disabled in code-navigation.json",
                    request.language.name()
                ));
            }
            let mut command = server_command(&request.project, &spec)?;
            let client = Client::spawn(
                &mut command,
                &uri(&request.project.root),
                spec.options,
                spec.settings,
                cancel,
            )
            .map_err(|e| {
                format!(
                    "{}: {e}\nInstall in WSL: {}",
                    request.language.name(),
                    request.language.install()
                )
            })?;
            state.servers.push(Server {
                root: request.project.root.clone(),
                distro: request.project.workspace.distro.clone(),
                language: request.language,
                client,
                documents: HashMap::new(),
                last_used: Instant::now(),
                started: Instant::now(),
            });
            state.servers.len() - 1
        };
        let server = &mut state.servers[index];
        server.last_used = Instant::now();
        let result = server.execute(request, cancel);
        // Timeouts and disconnects discard a possibly wedged process; cancellation
        // keeps the cache and lets the next request reuse successful indexing.
        if result.as_ref().is_err_and(|e| {
            e.contains("timed out")
                || e.contains("closed")
                || e.contains("exited")
                || e.contains("queue unavailable")
        }) {
            state.servers.remove(index);
        }
        result
    }
}
impl Server {
    fn call(&mut self, method: &str, params: Value, cancel: &Cancellation) -> Result<Value> {
        self.client
            .request(method, params, cancel, Duration::from_secs(30))
    }
    fn sync(&mut self, snapshots: &[Snapshot]) -> Result<()> {
        let keep = snapshots
            .iter()
            .filter(|s| {
                s.path.starts_with(&self.root) && Language::for_path(&s.path) == Some(self.language)
            })
            .map(|s| s.path.clone())
            .collect::<std::collections::HashSet<_>>();
        let closed = self
            .documents
            .keys()
            .filter(|p| !keep.contains(*p))
            .cloned()
            .collect::<Vec<_>>();
        for path in closed {
            self.client.notify(
                "textDocument/didClose",
                json!({"textDocument":{"uri":uri(&path)}}),
            )?;
            self.documents.remove(&path);
        }
        let sync = &self.client.capabilities["textDocumentSync"];
        let kind = sync
            .as_u64()
            .or_else(|| sync["change"].as_u64())
            .unwrap_or(1);
        for snapshot in snapshots.iter().filter(|s| keep.contains(&s.path)) {
            if let Some((text, version)) = self.documents.get_mut(&snapshot.path) {
                if *text == snapshot.text {
                    continue;
                }
                *version = version.checked_add(1).unwrap_or(1);
                let change = if kind == 2 {
                    json!({"range":{"start":{"line":0,"character":0},"end":position(text,text.len())},"text":snapshot.text})
                } else {
                    json!({"text":snapshot.text})
                };
                self.client.notify("textDocument/didChange",json!({"textDocument":{"uri":uri(&snapshot.path),"version":version},"contentChanges":[change]}))?;
                *text = snapshot.text.clone();
            } else {
                self.client.notify("textDocument/didOpen",json!({"textDocument":{"uri":uri(&snapshot.path),"languageId":self.language.language_id(&snapshot.path),"version":1,"text":snapshot.text}}))?;
                self.documents
                    .insert(snapshot.path.clone(), (snapshot.text.clone(), 1));
            }
        }
        Ok(())
    }
    fn supports(&self, key: &str) -> bool {
        let v = &self.client.capabilities[key];
        !v.is_null() && *v != false
    }
    fn incoming(&mut self, item: Value, depth: usize, cancel: &Cancellation) -> Result<Response> {
        if depth > 12 {
            return Err("Call hierarchy depth limit reached".into());
        }
        let incoming = self.call("callHierarchy/incomingCalls", json!({"item":item}), cancel)?;
        let mut result = vec![];
        for call in incoming.as_array().into_iter().flatten() {
            let from = &call["from"];
            let Some(mut loc) = location(from) else {
                continue;
            };
            if !loc.path.starts_with(&self.root) {
                continue;
            }
            let ranges = call["fromRanges"].as_array().cloned().unwrap_or_default();
            if let Some(range) = ranges
                .first()
                .and_then(|v| serde_json::from_value(v.clone()).ok())
            {
                loc.range = range;
            }
            result.push(Row {
                snippet: None,
                name: from["name"].as_str().unwrap_or("Caller").into(),
                detail: format!(
                    "{} · {} call site(s)",
                    from["detail"].as_str().unwrap_or(""),
                    ranges.len()
                ),
                location: loc,
                item: Some(from.clone()),
                depth,
            });
            if result.len() >= LIMIT {
                break;
            }
        }
        result.sort_by(|a, b| {
            (&a.location.path, a.location.range.start.line)
                .cmp(&(&b.location.path, b.location.range.start.line))
        });
        Ok(Response {
            rows: result,
            note: "Incoming calls · click a call site to open it; expand to follow callers".into(),
        })
    }
    fn execute(&mut self, request: Request, cancel: &Cancellation) -> Result<Response> {
        let mut request = request;
        if self.language == Language::TypeScript
            && !request.snapshots.iter().any(|s| {
                s.path.starts_with(&self.root) && Language::for_path(&s.path) == Some(self.language)
            })
        {
            if let Some(seed) = find_seed(&request.project, cancel)? {
                request.snapshots.push(seed);
            }
        }
        self.sync(&request.snapshots)?;
        cancel.check()?;
        match request.query {
            Query::Symbols(query) => {
                if !self.supports("workspaceSymbolProvider") {
                    return Err("This server does not support project symbol search".into());
                }
                let mut symbols = self.call("workspace/symbol", json!({"query":query}), cancel)?;
                // Some servers accept requests before their asynchronous initial
                // configuration/index load finishes. Briefly retry only during
                // startup, avoiding a false empty result on the first search.
                let mut probes = 0;
                while symbols.as_array().is_some_and(|items| items.is_empty())
                    && self.started.elapsed() < Duration::from_secs(5)
                    && probes < 6
                {
                    cancel.check()?;
                    std::thread::sleep(Duration::from_millis(150));
                    cancel.check()?;
                    symbols = self.call("workspace/symbol", json!({"query":query}), cancel)?;
                    probes += 1;
                }
                // LSP 3.17 may return URI-only symbols, resolved before navigation.
                let mut resolved = vec![];
                for symbol in symbols
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|v| v["kind"].as_u64().is_some_and(function))
                    .take(LIMIT)
                {
                    if symbol["location"].get("range").is_none() {
                        if symbol["location"]["uri"]
                            .as_str()
                            .and_then(|u| uri_path(u).ok())
                            .is_some_and(|p| p.starts_with(&self.root))
                        {
                            resolved.push(self.call(
                                "workspaceSymbol/resolve",
                                symbol.clone(),
                                cancel,
                            )?);
                        }
                    } else {
                        resolved.push(symbol.clone());
                    }
                }
                let rows = rows(Value::Array(resolved), Some(&self.root), true);
                Ok(Response {rows,note:"Project functions and methods · up to 500 results · narrow the query if needed".into()})
            }
            Query::Definition(pos, path) => {
                if !self.supports("definitionProvider") {
                    return Err("This server does not support go to definition".into());
                }
                let result = self.call(
                    "textDocument/definition",
                    json!({"textDocument":{"uri":uri(&path)},"position":pos}),
                    cancel,
                )?;
                Ok(Response {
                    rows: rows(result, None, false),
                    note: "Definitions".into(),
                })
            }
            Query::References(pos, path) => {
                self.references(pos, &path, cancel, "References (including declarations)")
            }
            Query::Callers(pos, path) => {
                if !self.supports("callHierarchyProvider") {
                    return self.references(pos,&path,cancel,"Call hierarchy unsupported by this server; showing references, which can include non-call usages");
                }
                let prepared = self.call(
                    "textDocument/prepareCallHierarchy",
                    json!({"textDocument":{"uri":uri(&path)},"position":pos}),
                    cancel,
                )?;
                let items = prepared.as_array().cloned().unwrap_or_default();
                let mut rows = Vec::new();
                for item in items.into_iter().take(LIMIT) {
                    let Some(loc) = location(&item) else { continue };
                    if !loc.path.starts_with(&self.root) {
                        continue;
                    }
                    rows.push(Row {
                        snippet: None,
                        name: item["name"].as_str().unwrap_or("Function").into(),
                        detail: "Callers of".into(),
                        location: loc,
                        item: Some(item.clone()),
                        depth: 0,
                    });
                    rows.extend(
                        self.incoming(item, 1, cancel)?
                            .rows
                            .into_iter()
                            .take(LIMIT - rows.len()),
                    );
                    if rows.len() >= LIMIT {
                        break;
                    }
                }
                Ok(Response {
                    rows,
                    note: "Incoming calls · expand a function to follow its callers".into(),
                })
            }
            Query::Incoming(item, depth) => self.incoming(item, depth, cancel),
        }
    }
    fn references(
        &mut self,
        pos: Position,
        path: &Path,
        cancel: &Cancellation,
        note: &str,
    ) -> Result<Response> {
        if !self.supports("referencesProvider") {
            return Err("This server does not support find references".into());
        }
        let result=self.call("textDocument/references",json!({"textDocument":{"uri":uri(path)},"position":pos,"context":{"includeDeclaration":true}}),cancel)?;
        let mut rows = rows(result, Some(&self.root), false);
        let snapshots = self
            .documents
            .iter()
            .map(|(path, (text, _))| Snapshot {
                path: path.clone(),
                text: text.clone(),
            })
            .collect::<Vec<_>>();
        attach_snippets(
            &mut rows,
            &Workspace::prepare(self.distro.clone(), self.root.clone())
                .map_err(|e| e.to_string())?,
            &snapshots,
            cancel,
        )?;
        Ok(Response {
            rows,
            note: note.into(),
        })
    }
}

fn attach_snippets(
    rows: &mut [Row],
    workspace: &Workspace,
    snapshots: &[Snapshot],
    cancel: &Cancellation,
) -> Result<()> {
    use std::io::Read;
    let mut cache: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut budget = 16 * 1024 * 1024usize;
    for row in rows {
        cancel.check()?;
        let text = cache.entry(row.location.path.clone()).or_insert_with(|| {
            if let Some(snapshot) = snapshots.iter().find(|s| s.path == row.location.path) {
                return Some(snapshot.text.clone());
            }
            let path = workspace.host_path(&row.location.path).ok()?;
            let length = std::fs::metadata(&path).ok()?.len() as usize;
            if length > 2 * 1024 * 1024 || length > budget {
                return None;
            }
            budget -= length;
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .ok()?
                .take((length + 1) as u64)
                .read_to_end(&mut bytes)
                .ok()?;
            String::from_utf8(bytes).ok()
        });
        let Some(text) = text else { continue };
        let Some(line) = text.lines().nth(row.location.range.start.line as usize) else {
            continue;
        };
        let start = offset(
            line,
            Position {
                line: 0,
                character: row.location.range.start.character,
            },
        );
        let end = if row.location.range.end.line == row.location.range.start.line {
            offset(
                line,
                Position {
                    line: 0,
                    character: row.location.range.end.character,
                },
            )
        } else {
            line.len()
        };
        // Keep a bounded preview around the occurrence, including very long/minified lines.
        let left = line[..start]
            .char_indices()
            .rev()
            .nth(80)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let right = line[start..]
            .char_indices()
            .nth(240)
            .map(|(i, _)| start + i)
            .unwrap_or(line.len());
        let preview = &line[left..right];
        let trim = preview.len() - preview.trim_start().len();
        row.snippet = Some(Snippet {
            text: preview.trim_start().to_string(),
            highlight: start.saturating_sub(left + trim)
                ..end
                    .min(right)
                    .saturating_sub(left + trim)
                    .max(start.saturating_sub(left + trim)),
        });
    }
    Ok(())
}

// The WSL supervisor owns the server's process group. Losing the Windows pipe
// closes the group too, even if an unresponsive server never reads stdin.
const SUPERVISOR: &str = r#"
import os, sys, subprocess, selectors, signal
try: os.nice(5)
except OSError: pass
p = subprocess.Popen(sys.argv[1:], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True)
s = selectors.DefaultSelector()
source_to_target = {sys.stdin.fileno(): p.stdin.fileno(), p.stdout.fileno(): sys.stdout.fileno()}
pending = {target: bytearray() for target in source_to_target.values()}
for source, target in source_to_target.items():
    os.set_blocking(source, False)
    os.set_blocking(target, False)
    s.register(source, selectors.EVENT_READ, ('read', target))
try:
    while True:
        for key, events in s.select():
            kind, target = key.data
            if kind == 'read':
                try: data = os.read(key.fd, 65536)
                except BlockingIOError: continue
                if not data: raise EOFError()
                buffer = pending[target]
                if not buffer: s.register(target, selectors.EVENT_WRITE, ('write', target))
                buffer.extend(data)
                if len(buffer) > 32 * 1024 * 1024: raise BufferError()
            else:
                buffer = pending[target]
                try: written = os.write(target, buffer)
                except BlockingIOError: continue
                del buffer[:written]
                if not buffer: s.unregister(target)
except (EOFError, BrokenPipeError, BufferError, OSError):
    pass
finally:
    try: os.killpg(p.pid, signal.SIGTERM)
    except ProcessLookupError: pass
    try: p.wait(timeout=0.5)
    except subprocess.TimeoutExpired:
        try: os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError: pass
        p.wait()
"#;
fn find_seed(project: &Project, cancel: &Cancellation) -> Result<Option<Snapshot>> {
    let host = project.workspace.host_root.join(
        project
            .root
            .strip_prefix(&project.workspace.linux_root)
            .map_err(|e| e.to_string())?,
    );
    let mut folders = std::collections::VecDeque::from([(host, project.root.clone())]);
    let mut visited = 0;
    while let Some((host, linux)) = folders.pop_front() {
        cancel.check()?;
        if visited >= 4096 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&host) else {
            continue;
        };
        for entry in entries {
            visited += 1;
            if visited > 4096 {
                break;
            }
            cancel.check()?;
            let Ok(entry) = entry else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let path = linux.join(&name);
            if kind.is_dir() {
                if !matches!(
                    name.to_str(),
                    Some("node_modules" | "target" | "dist" | "build" | "vendor")
                ) && !name.to_string_lossy().starts_with('.')
                {
                    folders.push_back((entry.path(), path));
                }
            } else if kind.is_file() && Language::for_path(&path) == Some(Language::TypeScript) {
                if entry
                    .metadata()
                    .is_ok_and(|m| m.len() <= crate::workspace::MAX_EDITABLE_BYTES)
                {
                    if let Ok(text) = std::fs::read_to_string(entry.path()) {
                        return Ok(Some(Snapshot { path, text }));
                    }
                }
            }
        }
    }
    Ok(None)
}
fn merge_json(base: &mut Value, overlay: &Value) {
    if let (Some(base), Some(overlay)) = (base.as_object_mut(), overlay.as_object()) {
        for (key, value) in overlay {
            if let Some(old) = base.get_mut(key) {
                merge_json(old, value);
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else if !overlay.is_null() {
        *base = overlay.clone();
    }
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn server_command(project: &Project, spec: &ServerSpec) -> Result<Command> {
    let args = std::iter::once(&spec.command)
        .chain(spec.args.iter())
        .map(|s| quote(s))
        .collect::<Vec<_>>()
        .join(" ");
    // A GUI-launched WSL login shell often does not run the interactive nvm
    // setup in .bashrc. Use its configured default only when Node is absent.
    let environment = r#"export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$HOME/go/bin:$HOME/.local/share/araseo/lsp/node_modules/.bin:$PATH"; if ! command -v node >/dev/null 2>&1 && [ -s "$HOME/.nvm/nvm.sh" ]; then . "$HOME/.nvm/nvm.sh" --no-use; nvm use --silent default >/dev/null 2>&1 || true; fi; "#;
    let script = format!(
        "{environment}exec python3 -u -c {} {}",
        quote(SUPERVISOR),
        args
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
        command.args([
            "-d",
            &project.workspace.distro,
            "--cd",
            &project.root.to_string_lossy(),
            "--exec",
            "bash",
            "-lc",
            &script,
        ]);
        command.creation_flags(0x08000000);
        Ok(command)
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("bash");
        command.args(["-lc", &script]).current_dir(&project.root);
        Ok(command)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Visit {
    pub tab: Option<u32>,
    pub location: Location,
    pub scroll: Option<(f32, f32)>,
}
#[derive(Default)]
pub struct History {
    back: Vec<Visit>,
    forward: Vec<Visit>,
}
impl History {
    pub fn push(&mut self, visit: Visit) {
        if self.back.last() != Some(&visit) {
            self.back.push(visit);
            if self.back.len() > 200 {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }
    pub fn back(&mut self, current: Visit) -> Option<Visit> {
        let target = self.back.pop()?;
        self.forward.push(current);
        Some(target)
    }
    pub fn forward(&mut self, current: Visit) -> Option<Visit> {
        let target = self.forward.pop()?;
        self.back.push(current);
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_and_file_uris_round_trip_korean_spaces_and_astral_characters() {
        let path = Path::new("/project/한글 #😀%.rs");
        assert_eq!(uri_path(&uri(path)).unwrap(), path);
        let text = "abc\r\n한😀글();\n";
        let byte = text.find('글').unwrap();
        let pos = position(text, byte);
        assert_eq!(
            pos,
            Position {
                line: 1,
                character: 3
            }
        );
        assert_eq!(offset(text, pos), byte);
        assert_eq!(
            offset(
                text,
                Position {
                    line: 1,
                    character: 2
                }
            ),
            text.find('😀').unwrap()
        );
        assert!(uri_path("file://other-host/etc/passwd").is_err());
        assert!(uri_path("file:///project/%2E%2E/secret").is_err());
    }
    #[test]
    fn project_symbols_exclude_sibling_projects_and_non_functions() {
        let make = |name: &str, kind: u64, path: &str| json!({"name":name,"kind":kind,"location":{"uri":uri(Path::new(path)),"range":Range::default()}});
        let result = rows(
            json!([
                make("run", 12, "/p/a.rs"),
                make("other", 12, "/p2/a.rs"),
                make("Class", 5, "/p/a.rs"),
                make("run", 12, "/p/a.rs")
            ]),
            Some(Path::new("/p")),
            true,
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "run");
    }
    #[test]
    fn nearest_project_does_not_expand_to_home_or_neighbor_repository() {
        let base = std::env::temp_dir().join(format!("araseo-project-{}", std::process::id()));
        std::fs::create_dir_all(base.join("app/src")).unwrap();
        std::fs::create_dir_all(base.join("neighbor")).unwrap();
        std::fs::write(base.join("app/Cargo.toml"), "").unwrap();
        let workspace = Workspace {
            distro: "test".into(),
            linux_root: base.clone(),
            host_root: base.clone(),
        };
        assert_eq!(
            project_for(&workspace, &base.join("app/src/main.rs"), false)
                .unwrap()
                .root,
            base.join("app")
        );
        assert_eq!(
            project_for(&workspace, &base.join("neighbor/file.py"), false)
                .unwrap()
                .root,
            base.join("neighbor")
        );
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn history_returns_actual_cursor_and_scroll_and_new_jump_clears_forward() {
        let visit = |line| Visit {
            tab: Some(line),
            location: Location {
                path: "/p/a.rs".into(),
                range: Range {
                    start: Position { line, character: 2 },
                    end: Position { line, character: 4 },
                },
            },
            scroll: Some((0., -(line as f32))),
        };
        let mut history = History::default();
        history.push(visit(1));
        history.push(visit(2));
        assert_eq!(history.back(visit(3)), Some(visit(2)));
        assert_eq!(history.back(visit(2)), Some(visit(1)));
        assert_eq!(history.forward(visit(1)), Some(visit(2)));
        history.push(visit(2));
        assert!(!history.can_forward());
    }
    #[cfg(unix)]
    #[test]
    fn semantic_requests_reuse_the_server_sync_unsaved_text_and_keep_project_scope() {
        let root = std::env::temp_dir().join(format!("araseo-lsp-fixture-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let fixture = root.join("server.py");
        std::fs::write(
            &fixture,
            include_str!("../harness/fixtures/navigation-server.py"),
        )
        .unwrap();
        let config = root.join("config.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({"Rust":{"command":"python3","args":["-u",fixture]}}))
                .unwrap(),
        )
        .unwrap();
        let workspace = Workspace {
            distro: "test".into(),
            linux_root: root.clone(),
            host_root: root.clone(),
        };
        let project = Project {
            workspace,
            root: root.clone(),
        };
        let path = root.join("main.rs");
        let service = Service::new(Some(config));
        let cancel = Cancellation::default();
        let request = |query, text: &str| Request {
            project: project.clone(),
            language: Language::Rust,
            query,
            snapshots: vec![Snapshot {
                path: path.clone(),
                text: text.into(),
            }],
        };
        let first = service
            .execute(
                request(Query::Symbols("run".into()), "fn run() {} // 한글 😀"),
                &cancel,
            )
            .unwrap();
        assert_eq!(first.rows.len(), 1);
        assert_eq!(first.rows[0].name, "run");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !root.join("idle-config-ready").exists() {
            assert!(
                Instant::now() < deadline,
                "server configuration must be answered while the UI is idle"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let edited = service
            .execute(
                request(Query::Symbols("edited".into()), "fn edited() {}"),
                &cancel,
            )
            .unwrap();
        assert_eq!(edited.rows[0].name, "edited");
        assert_eq!(
            first.rows[0].detail, edited.rows[0].detail,
            "server PID must be reused"
        );
        let definition = service
            .execute(
                request(
                    Query::Definition(Position::default(), path.clone()),
                    "fn edited() {}",
                ),
                &cancel,
            )
            .unwrap();
        assert_eq!(definition.rows[0].location.path, root.join("other.rs"));
        let references = service
            .execute(
                request(
                    Query::References(Position::default(), path.clone()),
                    "fn edited() {}",
                ),
                &cancel,
            )
            .unwrap();
        assert_eq!(references.rows.len(), 1);
        let callers = service
            .execute(
                request(
                    Query::Callers(Position::default(), path.clone()),
                    "fn edited() {}",
                ),
                &cancel,
            )
            .unwrap();
        assert_eq!(callers.rows[0].name, "run");
        assert_eq!(callers.rows[1].name, "caller");
        assert_eq!(callers.rows[1].depth, 1);
        let item = callers.rows[0].item.clone().unwrap();
        let expanded = service
            .execute(request(Query::Incoming(item, 1), "fn edited() {}"), &cancel)
            .unwrap();
        assert_eq!(expanded.rows[0].depth, 1);
        cancel.cancel();
        assert!(
            service
                .execute(request(Query::Symbols("".into()), ""), &cancel)
                .unwrap_err()
                .contains("cancelled")
        );
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reference_previews_use_unsaved_unicode_and_bound_long_lines() {
        let workspace = Workspace::prepare("test", "/nonexistent-preview".into()).unwrap();
        let path = workspace.linux_root.join("main.rs");
        let text = "한글 😀 target();\n".to_string() + &"x".repeat(10000) + "target()";
        let mut result = rows(
            json!([
                {"uri":uri(&path),"range":{"start":{"line":0,"character":6},"end":{"line":0,"character":12}}},
                {"uri":uri(&path),"range":{"start":{"line":1,"character":10000},"end":{"line":1,"character":10006}}}
            ]),
            Some(&workspace.linux_root),
            false,
        );
        attach_snippets(
            &mut result,
            &workspace,
            &[Snapshot { path, text }],
            &Cancellation::default(),
        )
        .unwrap();
        for row in result {
            let preview = row.snippet.unwrap();
            assert_eq!(&preview.text[preview.highlight], "target");
            assert!(preview.text.len() < 1024);
        }
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "requires the rust-analyzer rustup component"]
    fn real_rust_analyzer_finds_definitions_callers_and_project_functions() {
        let root =
            std::env::temp_dir().join(format!("araseo-real-rust-nav-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"navigation_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        )
        .unwrap();
        let text =
            "fn target() {}\nfn caller() { target(); }\nfn main() { caller(); }\nmod helpers;\n";
        std::fs::write(
            root.join("src/helpers.rs"),
            "pub fn unopened_function() {}\n",
        )
        .unwrap();
        let path = root.join("src/main.rs");
        std::fs::write(&path, text).unwrap();
        let project = Project {
            workspace: Workspace {
                distro: "test".into(),
                linux_root: root.clone(),
                host_root: root.clone(),
            },
            root: root.clone(),
        };
        let service = Service::default();
        let cancel = Cancellation::default();
        let request = |query| Request {
            project: project.clone(),
            language: Language::Rust,
            query,
            snapshots: vec![Snapshot {
                path: path.clone(),
                text: text.into(),
            }],
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let result = service
                .execute(request(Query::Symbols("target".into())), &cancel)
                .unwrap();
            if result.rows.iter().any(|r| r.name == "target") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "rust-analyzer did not index target: {}",
                result.note
            );
            std::thread::sleep(Duration::from_millis(200));
        }
        let unopened = service
            .execute(request(Query::Symbols("unopened_function".into())), &cancel)
            .unwrap();
        assert!(
            unopened
                .rows
                .iter()
                .any(|r| r.location.path == root.join("src/helpers.rs")),
            "Rust project search must include unopened files"
        );
        let result = service
            .execute(
                request(Query::Definition(
                    Position {
                        line: 1,
                        character: 16,
                    },
                    path.clone(),
                )),
                &cancel,
            )
            .unwrap();
        assert_eq!(result.rows[0].location.range.start.line, 0);
        let result = service
            .execute(
                request(Query::Callers(
                    Position {
                        line: 0,
                        character: 5,
                    },
                    path.clone(),
                )),
                &cancel,
            )
            .unwrap();
        assert!(result.rows.iter().any(|r| r.name == "caller"));
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "requires pyright and typescript-language-server in the user LSP directory"]
    fn real_python_and_typescript_navigation() {
        for (language, filename, text, callpos, defpos) in [
            (
                Language::Python,
                "main.py",
                "def target():\n    pass\ndef caller():\n    target()\ncaller()\n",
                Position {
                    line: 3,
                    character: 6,
                },
                Position {
                    line: 0,
                    character: 5,
                },
            ),
            (
                Language::TypeScript,
                "main.ts",
                "function target() {}\nfunction caller() { target(); }\ncaller();\n",
                Position {
                    line: 1,
                    character: 22,
                },
                Position {
                    line: 0,
                    character: 11,
                },
            ),
        ] {
            let root = std::env::temp_dir().join(format!(
                "araseo-real-nav-{:?}-{}",
                language,
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            if language == Language::Python {
                std::fs::write(root.join("pyrightconfig.json"), "{\"include\":[\".\"]}").unwrap();
            } else {
                std::fs::write(root.join("tsconfig.json"), "{\"include\":[\"*.ts\"]}").unwrap();
            }
            let helper = if language == Language::Python {
                "def unopened_function():\n    pass\n"
            } else {
                "export function unopened_function() {}\n"
            };
            let helper_path = root.join(if language == Language::Python {
                "helpers.py"
            } else {
                "helpers.ts"
            });
            std::fs::write(&helper_path, helper).unwrap();
            let path = root.join(filename);
            std::fs::write(&path, text).unwrap();
            let project = Project {
                workspace: Workspace {
                    distro: "test".into(),
                    linux_root: root.clone(),
                    host_root: root.clone(),
                },
                root: root.clone(),
            };
            let service = Service::default();
            let cancel = Cancellation::default();
            let request = |query| Request {
                project: project.clone(),
                language,
                query,
                snapshots: vec![Snapshot {
                    path: path.clone(),
                    text: text.into(),
                }],
            };
            let empty = service
                .execute(
                    Request {
                        project: project.clone(),
                        language,
                        query: Query::Symbols("target".into()),
                        snapshots: vec![],
                    },
                    &cancel,
                )
                .unwrap();
            let _ = empty;
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let result = service
                    .execute(
                        Request {
                            project: project.clone(),
                            language,
                            query: Query::Symbols("target".into()),
                            snapshots: vec![],
                        },
                        &cancel,
                    )
                    .unwrap();
                if result.rows.iter().any(|r| r.name == "target") {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{:?}: symbols not indexed: {}",
                    language,
                    result.note
                );
                std::thread::sleep(Duration::from_millis(150));
            }
            loop {
                let unopened = service
                    .execute(request(Query::Symbols("unopened_function".into())), &cancel)
                    .unwrap();
                if unopened.rows.iter().any(|r| r.location.path == helper_path) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{:?} project search must include unopened files: {:?}",
                    language,
                    unopened.rows
                );
                std::thread::sleep(Duration::from_millis(150));
            }
            let result = service
                .execute(request(Query::Definition(callpos, path.clone())), &cancel)
                .unwrap();
            assert_eq!(
                result.rows[0].location.range.start.line, 0,
                "{:?}",
                language
            );
            let end = position(text, text.rfind("target()").unwrap() + "target".len());
            let at_end = service
                .execute(request(Query::Definition(end, path.clone())), &cancel)
                .unwrap();
            assert!(
                !at_end.rows.is_empty(),
                "{:?}: definition should accept identifier end",
                language
            );
            let result = service
                .execute(request(Query::Callers(defpos, path.clone())), &cancel)
                .unwrap();
            assert!(
                result.rows.iter().any(|r| r.name == "caller"),
                "{:?}: {:?}",
                language,
                result.rows
            );
            drop(service);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
