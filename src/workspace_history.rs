//! Persist added workspace folders per WSL distribution and starting folder.

use std::fs;
use std::path::{Path, PathBuf};

const FORMAT: &str = "araseo-workspace-folders-v1";

pub fn settings_path(distro: &str, root: &Path) -> Option<PathBuf> {
    let fonts_path = crate::appearance::settings_path()?;
    let directory = fonts_path.parent()?.join("workspaces");
    let key = format!("{distro}\0{}", root.to_string_lossy());
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in key.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(directory.join(format!("{hash:016x}.conf")))
}

pub fn load(path: &Path, distro: &str, root: &Path) -> Vec<PathBuf> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines = contents.lines();
    if lines.next() != Some(FORMAT)
        || lines.next().and_then(decode) != Some(distro.to_owned())
        || lines.next().and_then(decode) != Some(root.to_string_lossy().into_owned())
    {
        return Vec::new();
    }
    let mut folders = Vec::new();
    for line in lines {
        if let Some(folder) = decode(line).map(PathBuf::from)
            && !folders.contains(&folder)
        {
            folders.push(folder);
        }
    }
    folders
}

pub fn save(path: &Path, distro: &str, root: &Path, folders: &[PathBuf]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut contents = format!(
        "{FORMAT}\n{}\n{}\n",
        encode(distro),
        encode(&root.to_string_lossy())
    );
    for folder in folders {
        contents.push_str(&encode(&folder.to_string_lossy()));
        contents.push('\n');
    }
    fs::write(path, contents)
}

fn encode(value: &str) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode(value: &str) -> Option<String> {
    if value.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        bytes.push(((high << 4) | low) as u8);
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_only_folders_for_the_matching_workspace() {
        let directory = std::env::temp_dir().join(format!(
            "araseo-workspace-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("history.conf");
        let root = Path::new("/home/user/main");
        let folders = vec![
            PathBuf::from("/home/user/한글 project"),
            PathBuf::from("/other/line\nbreak"),
        ];
        save(&path, "Ubuntu", root, &folders).unwrap();
        assert_eq!(load(&path, "Ubuntu", root), folders);
        assert!(load(&path, "Debian", root).is_empty());
        assert!(load(&path, "Ubuntu", Path::new("/home/user/elsewhere")).is_empty());
        save(&path, "Ubuntu", root, &folders[1..]).unwrap();
        assert_eq!(load(&path, "Ubuntu", root), folders[1..]);
        fs::write(&path, "corrupt\ncontents\n").unwrap();
        assert!(load(&path, "Ubuntu", root).is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}
