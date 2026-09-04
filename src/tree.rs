use crate::workspace::Workspace;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GitStatus {
    #[default]
    Clean,
    Modified,
    Untracked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlatNode {
    pub name: String,
    pub linux_path: PathBuf,
    pub depth: i32,
    pub is_directory: bool,
    pub is_expanded: bool,
    pub git_status: GitStatus,
}

/// Returns a compact, context-aware emoji for the file tree. Project identity
/// takes priority over special folder names so it is visible before expansion.
pub fn icon_for(
    name: &str,
    is_directory: bool,
    is_expanded: bool,
    project_kind: &str,
) -> &'static str {
    let lowercase = name.to_ascii_lowercase();
    if is_directory {
        return match project_kind {
            "git" => "🌿",
            "local" => "🧩",
            _ => folder_icon(&lowercase, is_expanded),
        };
    }

    file_icon(&lowercase)
}

fn folder_icon(name: &str, is_expanded: bool) -> &'static str {
    match name {
        "test" | "tests" | "spec" | "specs" | "__tests__" => "🧪",
        "doc" | "docs" | "documentation" => "📚",
        "asset" | "assets" | "image" | "images" | "media" | "public" | "static" => "🎨",
        "script" | "scripts" | "bin" | "tools" => "🛠️",
        "build" | "dist" | "out" | "release" | "target" => "📦",
        "config" | "configs" | ".config" | ".github" => "⚙️",
        ".vscode" | ".idea" => "⚙️",
        "src" | "source" | "sources" | "lib" => "💻",
        "vendor" | "node_modules" | ".venv" | "venv" => "🧱",
        "example" | "examples" | "sample" | "samples" | "demo" | "demos" => "💡",
        "migration" | "migrations" | "database" | "db" => "🗄️",
        "locale" | "locales" | "i18n" | "translations" => "🌐",
        _ if is_expanded => "📂",
        _ => "📁",
    }
}

fn file_icon(name: &str) -> &'static str {
    // Extension is the single source of truth whenever one exists. This keeps
    // README.md, AGENTS.md, and any other Markdown file visually consistent,
    // and applies the same rule to test files and package manifests.
    if let Some(extension) = file_extension(name) {
        return match extension {
            "rs" => "🦀",
            "py" | "pyw" => "🐍",
            "js" | "jsx" | "mjs" | "cjs" => "⚡",
            "ts" | "tsx" => "🔷",
            "go" => "🐹",
            "java" | "jar" => "☕",
            "rb" => "💎",
            "swift" => "🐦",
            "dart" => "🎯",
            "php" => "🐘",
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "cs" | "kt" | "kts" => "🧱",
            "html" | "htm" | "xml" => "🌐",
            "css" | "scss" | "sass" | "less" | "slint" => "🎨",
            "md" | "markdown" | "mdx" | "txt" | "rst" => "📝",
            "json" | "jsonc" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" => "⚙️",
            "env" | "pem" | "crt" | "cer" | "key" | "pfx" => "🔐",
            "lock" => "🔒",
            "gitignore" | "gitattributes" | "gitmodules" => "🙈",
            "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" => "🐚",
            "sql" | "db" | "sqlite" | "sqlite3" => "🗄️",
            "csv" | "tsv" | "xls" | "xlsx" | "parquet" => "📊",
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" => "🖼️",
            "mp3" | "wav" | "flac" | "ogg" | "m4a" => "🎵",
            "mp4" | "mov" | "avi" | "mkv" | "webm" => "🎬",
            "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => "📦",
            "pdf" => "📕",
            "log" => "📜",
            "ttf" | "otf" | "woff" | "woff2" => "🔤",
            _ => "📄",
        };
    }

    match name {
        "dockerfile" => "🐳",
        "makefile" | "justfile" | "taskfile" => "🛠️",
        ".gitignore" | ".gitattributes" | ".gitmodules" => "🙈",
        ".env" => "🔐",
        "readme" => "📖",
        "license" | "copying" => "⚖️",
        "changelog" | "history" => "📋",
        "agents" | "codex" | "claude" => "🤖",
        "gemfile" => "📦",
        _ => "📄",
    }
}

fn file_extension(name: &str) -> Option<&str> {
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty() && !extension.is_empty()).then_some(extension)
}

pub fn build_tree(
    workspace: &Workspace,
    expanded: &HashSet<PathBuf>,
    statuses: &HashMap<PathBuf, GitStatus>,
) -> Result<Vec<FlatNode>> {
    let mut output = Vec::new();
    append_directory(
        workspace,
        &workspace.host_root,
        0,
        expanded,
        statuses,
        &mut output,
    )?;
    Ok(output)
}

fn append_directory(
    workspace: &Workspace,
    directory: &Path,
    depth: i32,
    expanded: &HashSet<PathBuf>,
    statuses: &HashMap<PathBuf, GitStatus>,
    output: &mut Vec<FlatNode>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        let directory_rank = entry.file_type().map(|kind| !kind.is_dir()).unwrap_or(true);
        (
            directory_rank,
            entry.file_name().to_string_lossy().to_lowercase(),
        )
    });

    for entry in entries {
        if entry.file_name() == ".git" {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(value) => value,
            Err(_) => continue,
        };
        let host_path = entry.path();
        let linux_path = workspace.linux_path(&host_path)?;
        let is_directory = file_type.is_dir() && !file_type.is_symlink();
        let is_expanded = is_directory && expanded.contains(&linux_path);
        output.push(FlatNode {
            name: entry.file_name().to_string_lossy().into_owned(),
            linux_path: linux_path.clone(),
            depth,
            is_directory,
            is_expanded,
            git_status: statuses.get(&linux_path).copied().unwrap_or_default(),
        });
        if is_expanded {
            let _ = append_directory(workspace, &host_path, depth + 1, expanded, statuses, output);
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("araseo-tree-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn sorts_directories_first_and_only_expands_selected_directories() {
        let root = temporary_directory("layout");
        fs::create_dir_all(root.join("beta/nested")).unwrap();
        fs::create_dir_all(root.join("Alpha")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("zeta.txt"), "z").unwrap();
        fs::write(root.join("aardvark.txt"), "a").unwrap();

        let workspace = Workspace::new("Ubuntu", root.clone()).unwrap();
        let collapsed = build_tree(&workspace, &HashSet::new(), &HashMap::new()).unwrap();
        assert_eq!(
            collapsed.iter().map(|node| node.name.as_str()).collect::<Vec<_>>(),
            ["Alpha", "beta", "aardvark.txt", "zeta.txt"]
        );
        assert!(collapsed.iter().all(|node| node.depth == 0));

        let expanded = HashSet::from([root.join("beta")]);
        let tree = build_tree(&workspace, &expanded, &HashMap::new()).unwrap();
        let nested = tree.iter().find(|node| node.name == "nested").unwrap();
        assert_eq!(nested.depth, 1);
        assert!(!tree.iter().any(|node| node.name == ".git"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attaches_git_status_to_the_exact_file() {
        let root = temporary_directory("status");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("modified.rs"), "m").unwrap();
        fs::write(root.join("untracked.rs"), "u").unwrap();
        fs::write(root.join("clean.rs"), "c").unwrap();

        let workspace = Workspace::new("Ubuntu", root.clone()).unwrap();
        let statuses = HashMap::from([
            (root.join("modified.rs"), GitStatus::Modified),
            (root.join("untracked.rs"), GitStatus::Untracked),
        ]);
        let tree = build_tree(&workspace, &HashSet::new(), &statuses).unwrap();
        let status = |name: &str| {
            tree.iter()
                .find(|node| node.name == name)
                .unwrap()
                .git_status
        };
        assert_eq!(status("modified.rs"), GitStatus::Modified);
        assert_eq!(status("untracked.rs"), GitStatus::Untracked);
        assert_eq!(status("clean.rs"), GitStatus::Clean);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn assigns_context_aware_tree_icons() {
        assert_eq!(icon_for("repo", true, false, "git"), "🌿");
        assert_eq!(icon_for("project", true, false, "local"), "🧩");
        assert_eq!(icon_for("tests", true, false, ""), "🧪");
        assert_eq!(icon_for("ordinary", true, false, ""), "📁");
        assert_eq!(icon_for("ordinary", true, true, ""), "📂");
        assert_eq!(icon_for("main.rs", false, false, ""), "🦀");
        assert_eq!(icon_for("app.py", false, false, ""), "🐍");
        assert_eq!(icon_for("app.js", false, false, ""), "⚡");
        assert_ne!(
            icon_for("app.js", false, false, ""),
            icon_for("ordinary", true, false, "")
        );
        assert_eq!(icon_for("README.md", false, false, ""), "📝");
        assert_eq!(icon_for("AGENTS.md", false, false, ""), "📝");
        assert_eq!(icon_for("Cargo.toml", false, false, ""), "⚙️");
        assert_eq!(icon_for("button.test.ts", false, false, ""), "🔷");
        assert_eq!(icon_for("Dockerfile", false, false, ""), "🐳");
        assert_eq!(icon_for("archive.zip", false, false, ""), "📦");
        assert_eq!(icon_for("unknown.data", false, false, ""), "📄");
    }

    #[test]
    fn identical_extensions_always_receive_identical_icons() {
        for names in [
            ["README.md", "AGENTS.md", "guide.md"],
            ["app.py", "test_app.py", "script.py"],
            ["app.js", "eslint.config.js", "worker.js"],
            ["config.json", "package-lock.json", "data.json"],
            ["Cargo.toml", "pyproject.toml", "settings.toml"],
            ["app.ts", "button.test.ts", "types.ts"],
            ["config.yml", "docker-compose.yml", "pnpm-lock.yml"],
        ] {
            let expected = icon_for(names[0], false, false, "");
            assert!(names[1..]
                .iter()
                .all(|name| icon_for(name, false, false, "") == expected));
        }
    }
}
