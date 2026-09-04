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
        let linux_root_text = normalize_linux_path(&linux_root);
        if !linux_root_text.starts_with('/') {
            bail!("workspace path must be absolute: {}", linux_root.display());
        }
        if linux_root_text.split('/').any(|part| part == "..") {
            bail!("workspace path must not contain '..'");
        }

        let distro = distro.into();
        let host_root = linux_to_host_path(&distro, &linux_root);
        if !host_root.is_dir() {
            bail!("workspace is not accessible: {}", linux_root.display());
        }

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

pub fn linux_to_host_path(distro: &str, linux_path: &Path) -> PathBuf {
    if cfg!(windows) {
        let linux_path = normalize_linux_path(linux_path);
        let suffix = linux_path.trim_start_matches('/').replace('/', "\\");
        PathBuf::from(format!(r"\\wsl.localhost\{}\{}", distro, suffix))
    } else {
        linux_path.to_path_buf()
    }
}

fn normalize_linux_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_that_escapes_workspace() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("araseo-workspace-test-{}", std::process::id()));
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
