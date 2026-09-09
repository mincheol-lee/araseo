use std::path::{Path, PathBuf};

pub const DEFAULT_FONT_BRIGHTNESS: i32 = 120;

#[derive(Debug, PartialEq, Eq)]
pub struct FontSizes {
    pub terminal: i32,
    pub editor: i32,
    pub tree: i32,
    pub terminal_brightness: i32,
    pub editor_brightness: i32,
    pub tree_brightness: i32,
}

impl Default for FontSizes {
    fn default() -> Self {
        Self {
            terminal: 14,
            editor: 14,
            tree: 12,
            terminal_brightness: DEFAULT_FONT_BRIGHTNESS,
            editor_brightness: DEFAULT_FONT_BRIGHTNESS,
            tree_brightness: DEFAULT_FONT_BRIGHTNESS,
        }
    }
}

impl FontSizes {
    pub fn load(path: &Path) -> Self {
        let mut sizes = Self::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let Ok(value) = value.trim().parse::<i32>() else {
                    continue;
                };
                match key.trim() {
                    "terminal" => sizes.terminal = value.clamp(10, 28),
                    "editor" => sizes.editor = value.clamp(10, 28),
                    "tree" => sizes.tree = value.clamp(10, 24),
                    "terminal_brightness" => sizes.terminal_brightness = value.clamp(50, 150),
                    "editor_brightness" => sizes.editor_brightness = value.clamp(50, 150),
                    "tree_brightness" => sizes.tree_brightness = value.clamp(50, 150),
                    _ => {}
                }
            }
        }
        sizes
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            format!(
                "terminal={}\neditor={}\ntree={}\nterminal_brightness={}\neditor_brightness={}\ntree_brightness={}\n",
                self.terminal,
                self.editor,
                self.tree,
                self.terminal_brightness,
                self.editor_brightness,
                self.tree_brightness,
            ),
        )
    }
}

pub fn settings_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    base.map(|base| base.join("araseo").join("fonts.conf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_independent_sizes_and_recovers_from_invalid_settings() {
        let dir = std::env::temp_dir().join(format!(
            "araseo-fonts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("fonts.conf");
        assert_eq!(FontSizes::load(&path), FontSizes::default());
        let sizes = FontSizes {
            terminal: 20,
            editor: 16,
            tree: 13,
            terminal_brightness: 90,
            editor_brightness: 130,
            tree_brightness: 105,
        };
        sizes.save(&path).unwrap();
        assert_eq!(FontSizes::load(&path), sizes);
        std::fs::write(
            &path,
            "terminal=999\neditor=bad\ntree=-3\nterminal_brightness=999\neditor_brightness=bad\ntree_brightness=-3\nunknown=42",
        )
        .unwrap();
        assert_eq!(
            FontSizes::load(&path),
            FontSizes {
                terminal: 28,
                editor: 14,
                tree: 10,
                terminal_brightness: 150,
                editor_brightness: DEFAULT_FONT_BRIGHTNESS,
                tree_brightness: 50,
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
