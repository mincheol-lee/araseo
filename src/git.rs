use crate::tree::GitStatus;
use crate::workspace::Workspace;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const INOTIFY_MONITOR: &str = r#"
import ctypes
import os
import select
import struct
import subprocess
import sys
import time

root = os.path.abspath(sys.argv[1])
excluded = {'target', 'node_modules', '.cache'}

IN_MODIFY = 0x00000002
IN_ATTRIB = 0x00000004
IN_CLOSE_WRITE = 0x00000008
IN_MOVED_FROM = 0x00000040
IN_MOVED_TO = 0x00000080
IN_CREATE = 0x00000100
IN_DELETE = 0x00000200
IN_DELETE_SELF = 0x00000400
IN_MOVE_SELF = 0x00000800
IN_IGNORED = 0x00008000
IN_ISDIR = 0x40000000
WATCH_MASK = (IN_MODIFY | IN_ATTRIB | IN_CLOSE_WRITE | IN_MOVED_FROM |
              IN_MOVED_TO | IN_CREATE | IN_DELETE | IN_DELETE_SELF | IN_MOVE_SELF)

libc = ctypes.CDLL(None, use_errno=True)
libc.inotify_init1.argtypes = [ctypes.c_int]
libc.inotify_init1.restype = ctypes.c_int
libc.inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
libc.inotify_add_watch.restype = ctypes.c_int

fd = libc.inotify_init1(os.O_CLOEXEC | os.O_NONBLOCK)
if fd < 0:
    error = os.strerror(ctypes.get_errno()).encode('utf-8', 'replace')
    print('E ' + error.hex(), flush=True)
    sys.exit(1)

watches = {}

def report_error(message):
    print('E ' + str(message).encode('utf-8', 'replace').hex(), flush=True)

def add_watch(path):
    encoded = os.fsencode(path)
    wd = libc.inotify_add_watch(fd, encoded, WATCH_MASK)
    if wd < 0:
        report_error('Cannot watch ' + path + ': ' + os.strerror(ctypes.get_errno()))
        return
    watches[wd] = path

def add_tree(path):
    if os.path.basename(path) in excluded or os.path.basename(path) == '.git':
        return
    add_watch(path)
    for current, directories, _ in os.walk(path, followlinks=False):
        directories[:] = [name for name in directories if name not in excluded and name != '.git']
        if current != path:
            add_watch(current)

def repositories():
    found = []
    for current, directories, files in os.walk(root, followlinks=False):
        if '.git' in directories or '.git' in files:
            found.append(current)
        directories[:] = [name for name in directories if name not in excluded and name != '.git']
    return found

def emit_status():
    print('B', flush=True)
    for repository in repositories():
        result = subprocess.run(
            ['git', '-C', repository, 'status', '--porcelain=v2', '-z', '--untracked-files=all'],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode == 0:
            relative = os.path.relpath(repository, root)
            print('R ' + os.fsencode(relative).hex() + ' ' + result.stdout.hex(), flush=True)
    print('Z', flush=True)

add_tree(root)
emit_status()

poller = select.poll()
poller.register(fd, select.POLLIN)
poller.register(sys.stdin.fileno(), select.POLLIN | select.POLLHUP)
event_header = struct.Struct('iIII')
dirty = False
deadline = None

while True:
    timeout = -1
    if deadline is not None:
        timeout = max(0, int((deadline - time.monotonic()) * 1000))
    events = poller.poll(timeout)
    for source, mask in events:
        if source == sys.stdin.fileno():
            command = sys.stdin.readline()
            if not command or command.strip() == 'quit':
                sys.exit(0)
            emit_status()
            dirty = False
            deadline = None
            continue
        if source != fd or not (mask & select.POLLIN):
            continue
        try:
            data = os.read(fd, 1024 * 1024)
        except BlockingIOError:
            continue
        offset = 0
        while offset + event_header.size <= len(data):
            wd, event_mask, cookie, name_length = event_header.unpack_from(data, offset)
            offset += event_header.size
            raw_name = data[offset:offset + name_length]
            offset += name_length
            name = os.fsdecode(raw_name.split(b'\0', 1)[0]) if name_length else ''
            parent = watches.get(wd, root)
            if event_mask & IN_IGNORED:
                watches.pop(wd, None)
            if name in excluded:
                continue
            if name == '.git':
                dirty = True
                deadline = time.monotonic() + 0.4
                continue
            if event_mask & IN_ISDIR and event_mask & (IN_CREATE | IN_MOVED_TO):
                child = os.path.join(parent, name)
                if os.path.isdir(child):
                    add_tree(child)
            dirty = True
            deadline = time.monotonic() + 0.4
    if dirty and deadline is not None and time.monotonic() >= deadline:
        emit_status()
        dirty = False
        deadline = None
"#;

pub struct StatusMonitor {
    child: Child,
    input: ChildStdin,
    receiver: Receiver<Result<Vec<(Vec<u8>, Vec<u8>)>, String>>,
    root: PathBuf,
}

impl StatusMonitor {
    pub fn spawn(workspace: &Workspace) -> std::io::Result<Self> {
        let mut command = if cfg!(windows) {
            let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
            command
                .args(["-d", &workspace.distro, "--exec", "/usr/bin/python3", "-u", "-c"])
                .arg(INOTIFY_MONITOR)
                .arg(&workspace.linux_root);
            command
        } else {
            let mut command = Command::new("/usr/bin/python3");
            command
                .args(["-u", "-c", INOTIFY_MONITOR])
                .arg(&workspace.host_root);
            command
        };
        hide_window(&mut command);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = command.spawn()?;
        let input = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "monitor stdin unavailable")
        })?;
        let output = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "monitor stdout unavailable")
        })?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut pending = None;
            for line in BufReader::new(output).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        break;
                    }
                };
                match decode_monitor_line(&line) {
                    Ok(MonitorLine::Begin) => pending = Some(Vec::new()),
                    Ok(MonitorLine::Repository(path, status)) => {
                        if let Some(batch) = pending.as_mut() {
                            batch.push((path, status));
                        }
                    }
                    Ok(MonitorLine::End) => {
                        if let Some(batch) = pending.take()
                            && sender.send(Ok(batch)).is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        if sender.send(Err(error)).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            child,
            input,
            receiver,
            root: workspace.linux_root.clone(),
        })
    }

    pub fn poll_latest(&mut self) -> Option<Result<StatusSnapshot, String>> {
        let mut latest = None;
        while let Ok(message) = self.receiver.try_recv() {
            latest = Some(message.map(|repositories| {
                let mut snapshot = StatusSnapshot::default();
                for (relative_path, bytes) in repositories {
                    let relative_path = PathBuf::from(String::from_utf8_lossy(&relative_path).as_ref());
                    let repository = if relative_path == PathBuf::from(".") {
                        self.root.clone()
                    } else {
                        self.root.join(relative_path)
                    };
                    snapshot.repositories.insert(repository.clone());
                    snapshot.statuses.extend(parse_porcelain(&repository, &bytes));
                }
                snapshot
            }));
        }
        latest
    }

    pub fn force_refresh(&mut self) -> std::io::Result<()> {
        self.input.write_all(b"refresh\n")?;
        self.input.flush()
    }
}

#[derive(Default)]
pub struct StatusSnapshot {
    pub statuses: HashMap<PathBuf, GitStatus>,
    pub repositories: HashSet<PathBuf>,
}

impl Drop for StatusMonitor {
    fn drop(&mut self) {
        let _ = self.input.write_all(b"quit\n");
        let _ = self.input.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn read_status(workspace: &Workspace) -> HashMap<PathBuf, GitStatus> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
        command
            .args(["-d", &workspace.distro, "--cd"])
            .arg(&workspace.linux_root)
            .args([
                "git",
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=all",
            ]);
        command
    } else {
        let mut command = Command::new("git");
        command.arg("-C")
            .arg(&workspace.host_root)
            .args(["status", "--porcelain=v2", "-z", "--untracked-files=all"]);
        command
    };
    hide_window(&mut command);
    let output = command.output();

    let Ok(output) = output else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    parse_porcelain(&workspace.linux_root, &output.stdout)
}

#[derive(Debug)]
enum MonitorLine {
    Begin,
    Repository(Vec<u8>, Vec<u8>),
    End,
}

fn decode_monitor_line(line: &str) -> Result<MonitorLine, String> {
    if line == "B" {
        return Ok(MonitorLine::Begin);
    }
    if line == "Z" {
        return Ok(MonitorLine::End);
    }
    if let Some(payload) = line.strip_prefix("E ") {
        let bytes = decode_hex(payload)?;
        return Err(String::from_utf8_lossy(&bytes).trim().to_string());
    }
    if let Some(payload) = line.strip_prefix("R ") {
        let (path, status) = payload
            .split_once(' ')
            .ok_or_else(|| "invalid Git repository response".to_string())?;
        return Ok(MonitorLine::Repository(decode_hex(path)?, decode_hex(status)?));
    }
    Err("invalid Git monitor response type".into())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("invalid Git monitor hex payload".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| "invalid hex payload")?;
            u8::from_str_radix(text, 16).map_err(|_| "invalid hex payload")
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(str::to_string)
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}

fn parse_porcelain(root: &std::path::Path, bytes: &[u8]) -> HashMap<PathBuf, GitStatus> {
    let mut statuses = HashMap::new();
    let mut records = bytes.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        let (status, path) = match record.first().copied() {
            Some(b'?') if record.len() > 2 => (GitStatus::Untracked, &record[2..]),
            Some(b'1') => {
                let Some(path_start) = nth_space(record, 8) else {
                    continue;
                };
                (GitStatus::Modified, &record[path_start + 1..])
            }
            Some(b'2') => {
                let Some(path_start) = nth_space(record, 9) else {
                    continue;
                };
                let _original_path = records.next();
                (GitStatus::Modified, &record[path_start + 1..])
            }
            _ => continue,
        };
        statuses.insert(root.join(String::from_utf8_lossy(path).as_ref()), status);
    }
    statuses
}

fn nth_space(bytes: &[u8], count: usize) -> Option<usize> {
    bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b' ')
        .nth(count - 1)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_changed_and_untracked_files() {
        let root = PathBuf::from("/work");
        let parsed = parse_porcelain(
            &root,
            b"1 .M N... 100644 100644 100644 abcdef abcdef src/main.rs\0? new file.txt\0",
        );
        assert_eq!(parsed[&root.join("src/main.rs")], GitStatus::Modified);
        assert_eq!(parsed[&root.join("new file.txt")], GitStatus::Untracked);
    }

    #[test]
    fn decodes_monitor_status_and_errors() {
        let MonitorLine::Repository(path, status) =
            decode_monitor_line("R 7265706f 3f206e65772e74787400").unwrap()
        else {
            panic!("expected repository response");
        };
        assert_eq!(path, b"repo");
        assert_eq!(status, b"? new.txt\0");
        assert_eq!(decode_monitor_line("E 6661696c6564").unwrap_err(), "failed");
        assert!(decode_monitor_line("R 0 ").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn monitors_multiple_nested_repositories_with_inotify() {
        use std::fs;
        use std::path::Path;
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "araseo-git-monitor-{}-{nonce}",
            std::process::id()
        ));
        let first = root.join("first-repository");
        let second = root.join("second-repository");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::create_dir_all(root.join("local-files")).unwrap();
        initialise_repository(&first, "first.txt");
        initialise_repository(&second, "second.txt");

        let workspace = Workspace::new("Ubuntu", root.clone()).unwrap();
        let mut monitor = StatusMonitor::spawn(&workspace).unwrap();
        let initial = wait_for_snapshot(&mut monitor, |snapshot| {
            snapshot.repositories.contains(&first)
                && snapshot.repositories.contains(&second)
        });
        assert_eq!(initial.repositories.len(), 2);
        assert!(!initial.repositories.contains(&root.join("local-files")));

        fs::write(first.join("first.txt"), "modified\n").unwrap();
        fs::write(second.join("new.txt"), "untracked\n").unwrap();
        let changed = wait_for_snapshot(&mut monitor, |snapshot| {
            snapshot.statuses.get(&first.join("first.txt")) == Some(&GitStatus::Modified)
                && snapshot.statuses.get(&second.join("new.txt")) == Some(&GitStatus::Untracked)
        });
        assert_eq!(
            changed.statuses[&first.join("first.txt")],
            GitStatus::Modified
        );
        assert_eq!(
            changed.statuses[&second.join("new.txt")],
            GitStatus::Untracked
        );

        drop(monitor);
        fs::remove_dir_all(root).unwrap();

        fn initialise_repository(repository: &Path, tracked_name: &str) {
            fs::write(repository.join(tracked_name), "initial\n").unwrap();
            for arguments in [
                vec!["init", "--quiet"],
                vec!["config", "user.email", "araseo-harness@example.invalid"],
                vec!["config", "user.name", "Araseo Harness"],
                vec!["add", "."],
                vec!["commit", "--quiet", "-m", "initial"],
            ] {
                let output = Command::new("git")
                    .arg("-C")
                    .arg(repository)
                    .args(arguments)
                    .output()
                    .expect("Git must be installed for the Araseo harness");
                assert!(
                    output.status.success(),
                    "Git setup failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }

        fn wait_for_snapshot(
            monitor: &mut StatusMonitor,
            predicate: impl Fn(&StatusSnapshot) -> bool,
        ) -> StatusSnapshot {
            let deadline = Instant::now() + Duration::from_secs(8);
            while Instant::now() < deadline {
                if let Some(message) = monitor.poll_latest() {
                    let snapshot = message.expect("Git monitor returned an error");
                    if predicate(&snapshot) {
                        return snapshot;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("timed out waiting for the Git/inotify snapshot");
        }
    }
}
