use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const MAX_EDITABLE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub distro: String,
    pub linux_root: PathBuf,
    pub host_root: PathBuf,
}

impl Workspace {
    pub fn new(distro: impl Into<String>, linux_root: PathBuf) -> Result<Self> {
        let workspace = Self::prepare(distro, linux_root)?;
        if !workspace.host_root.is_dir() {
            bail!(
                "workspace is not accessible: {}",
                workspace.linux_root.display()
            );
        }
        Ok(workspace)
    }

    /// Validate and map the path without touching WSL or the filesystem. The
    /// desktop can display its first frame while workers check accessibility.
    pub fn prepare(distro: impl Into<String>, linux_root: PathBuf) -> Result<Self> {
        let linux_root_text = normalize_linux_path(&linux_root);
        if !linux_root_text.starts_with('/') {
            bail!("workspace path must be absolute: {}", linux_root.display());
        }
        if linux_root_text.split('/').any(|part| part == "..") {
            bail!("workspace path must not contain '..'");
        }

        let distro = distro.into();
        let host_root = linux_to_host_path(&distro, &linux_root);
        Ok(Self {
            distro,
            linux_root,
            host_root,
        })
    }

    pub fn host_path(&self, linux_path: &Path) -> Result<PathBuf> {
        let relative = linux_path
            .strip_prefix(&self.linux_root)
            .with_context(|| format!("path is outside workspace: {}", linux_path.display()))?;
        if relative
            .components()
            .any(|part| part == Component::ParentDir)
        {
            bail!("path escapes workspace");
        }
        let candidate = self.host_root.join(relative);
        if candidate.exists() {
            let canonical_root = fs::canonicalize(&self.host_root)?;
            let canonical_candidate = fs::canonicalize(&candidate)?;
            if !canonical_candidate.starts_with(&canonical_root) {
                bail!("path resolves outside workspace: {}", linux_path.display());
            }
        }
        Ok(candidate)
    }

    pub fn linux_path(&self, host_path: &Path) -> Result<PathBuf> {
        let relative = host_path
            .strip_prefix(&self.host_root)
            .with_context(|| format!("path is outside workspace: {}", host_path.display()))?;
        Ok(self.linux_root.join(relative))
    }
}

/// Select the most specific workspace containing a Linux path. This also
/// handles a folder explicitly added below an existing workspace root.
pub fn for_path<'a>(
    workspaces: impl IntoIterator<Item = &'a Workspace>,
    path: &Path,
) -> Option<&'a Workspace> {
    workspaces
        .into_iter()
        .filter(|workspace| path.starts_with(&workspace.linux_root))
        .max_by_key(|workspace| workspace.linux_root.components().count())
}

pub fn linux_to_host_path(distro: &str, linux_path: &Path) -> PathBuf {
    if cfg!(windows) {
        let linux_path = normalize_linux_path(linux_path);
        let suffix = linux_path.trim_start_matches('/').replace('/', "\\");
        PathBuf::from(format!(r"\\wsl.localhost\{}\{}", distro, suffix))
    } else {
        linux_path.to_path_buf()
    }
}

/// Translate a folder selected under the Explorer WSL network share. Other
/// Windows paths are handled by `wslpath` in the native picker.
pub fn wsl_unc_to_linux_path(distro: &str, selected: &str) -> Result<Option<PathBuf>> {
    let normalized = selected.replace('/', "\\");
    let Some(rest) = normalized.strip_prefix("\\\\") else {
        return Ok(None);
    };
    let mut parts = rest.split('\\');
    let server = parts.next().unwrap_or_default();
    if !server.eq_ignore_ascii_case("wsl.localhost") && !server.eq_ignore_ascii_case("wsl$") {
        return Ok(None);
    }
    let selected_distro = parts.next().unwrap_or_default();
    if !selected_distro.eq_ignore_ascii_case(distro) {
        bail!("select a folder in the current WSL distribution ({distro})");
    }
    let components = parts.filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if components.iter().any(|part| *part == "." || *part == "..") {
        bail!("selected WSL folder contains an invalid path segment");
    }
    Ok(Some(PathBuf::from(format!("/{}", components.join("/")))))
}

/// Convert an Explorer file path into a path understood by the current WSL shell.
/// This is deliberately independent of the workspace: a dropped file need not
/// be copied into the project before Codex can read it.
pub fn dropped_file_linux_path(distro: &str, source: &Path) -> Result<PathBuf> {
    let selected = source.to_string_lossy();
    if let Some(path) = wsl_unc_to_linux_path(distro, &selected)? {
        return Ok(path);
    }
    let bytes = selected.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        bail!("dropped file must be on a Windows drive or in the current WSL distribution");
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    let suffix = selected[3..].replace('\\', "/");
    Ok(PathBuf::from(format!("/mnt/{drive}/{suffix}")))
}

fn normalize_linux_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparing_a_workspace_does_not_require_disk_access() {
        let root = PathBuf::from("/araseo-nonexistent-workspace-for-path-test");
        assert_eq!(
            Workspace::prepare("Ubuntu", root.clone())
                .unwrap()
                .linux_root,
            root
        );
        assert!(Workspace::prepare("Ubuntu", PathBuf::from("relative")).is_err());
        assert!(Workspace::prepare("Ubuntu", PathBuf::from("/workspace/../escape")).is_err());
    }

    #[test]
    fn linux_path_is_unchanged_on_linux() {
        let path = Path::new("/home/user/한글 project");
        if cfg!(windows) {
            assert!(
                linux_to_host_path("Ubuntu", path)
                    .to_string_lossy()
                    .contains("wsl.localhost")
            );
        } else {
            assert_eq!(linux_to_host_path("Ubuntu", path), path);
        }
    }

    #[test]
    fn recognizes_linux_paths_independently_of_host_rules() {
        assert!(normalize_linux_path(Path::new("/home/user/project")).starts_with('/'));
        assert!(
            normalize_linux_path(Path::new("/home/user/../secret"))
                .split('/')
                .any(|part| part == "..")
        );
    }

    #[test]
    fn selects_the_most_specific_added_folder() {
        let parent = Workspace::prepare("Ubuntu", PathBuf::from("/projects")).unwrap();
        let other = Workspace::prepare("Ubuntu", PathBuf::from("/elsewhere/app")).unwrap();
        let nested = Workspace::prepare("Ubuntu", PathBuf::from("/projects/app")).unwrap();
        let workspaces = [parent, other, nested];
        assert_eq!(
            for_path(&workspaces, Path::new("/projects/app/src/main.rs"))
                .unwrap()
                .linux_root,
            Path::new("/projects/app")
        );
        assert_eq!(
            for_path(&workspaces, Path::new("/elsewhere/app/src/main.rs"))
                .unwrap()
                .linux_root,
            Path::new("/elsewhere/app")
        );
        assert!(for_path(&workspaces, Path::new("/missing/file")).is_none());
    }

    #[test]
    fn converts_explorer_wsl_folder_paths_and_rejects_other_distributions() {
        assert_eq!(
            wsl_unc_to_linux_path("Ubuntu", r"\\wsl.localhost\Ubuntu\home\user\한글 project")
                .unwrap(),
            Some(PathBuf::from("/home/user/한글 project"))
        );
        assert_eq!(
            wsl_unc_to_linux_path("Ubuntu", r"\\wsl$\ubuntu\home\user").unwrap(),
            Some(PathBuf::from("/home/user"))
        );
        assert_eq!(
            wsl_unc_to_linux_path("Ubuntu", r"C:\Projects\app").unwrap(),
            None
        );
        assert!(wsl_unc_to_linux_path("Ubuntu", r"\\wsl.localhost\Debian\home\user").is_err());
        assert!(wsl_unc_to_linux_path("Ubuntu", r"\\wsl.localhost\Ubuntu\..\user").is_err());
    }

    #[test]
    fn dropped_explorer_file_can_be_referenced_without_a_workspace_copy() {
        assert_eq!(
            dropped_file_linux_path("Ubuntu", Path::new(r"C:\Users\Minch\한글 file.txt")).unwrap(),
            PathBuf::from("/mnt/c/Users/Minch/한글 file.txt")
        );
        assert_eq!(
            dropped_file_linux_path(
                "Ubuntu",
                Path::new(r"\\wsl.localhost\Ubuntu\home\minch\file.txt")
            )
            .unwrap(),
            PathBuf::from("/home/minch/file.txt")
        );
        assert!(dropped_file_linux_path("Ubuntu", Path::new(r"C:relative.txt")).is_err());
        assert!(dropped_file_linux_path("Ubuntu", Path::new(r"\\server\share\file.txt")).is_err());
        assert!(
            dropped_file_linux_path("Ubuntu", Path::new(r"\\wsl.localhost\Debian\home\file.txt"))
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_that_escapes_workspace() {
        use std::os::unix::fs::symlink;

        let base =
            std::env::temp_dir().join(format!("araseo-workspace-test-{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside.txt");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root.join("escape.txt")).unwrap();

        let workspace = Workspace::new("Ubuntu", root.clone()).unwrap();
        assert!(workspace.host_path(&root.join("escape.txt")).is_err());
        fs::remove_dir_all(&base).unwrap();
    }
}
