//! A private persistent resize pipe, separate from terminal input and output.
use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// Only fixed numeric dimensions enter this protocol. The TTY is read again on
// each request so a terminal that is still starting can recover on the next one.
const RESIZE_SCRIPT: &str = r#"
while IFS=' ' read -r rows cols; do
    case "$rows:$cols" in *[!0-9:]*|:*|*:) printf 0; continue;; esac
    if IFS= read -r tty < "$1" && /usr/bin/stty -F "$tty" rows "$rows" cols "$cols" 2>/dev/null; then
        printf 1
    else
        printf 0
    fi
done
"#;

struct ResizePipe {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<io::Result<u8>>,
}

impl ResizePipe {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take();
        let mut output = child.stdout.take().expect("piped resize stdout");
        let (sender, replies) = mpsc::sync_channel(1);
        let pipe = Self {
            child,
            input,
            replies,
        };
        // If thread creation fails, pipe's Drop closes stdin and reaps the child.
        std::thread::Builder::new()
            .name("araseo-resize-reply".into())
            .spawn(move || {
                loop {
                    let mut byte = [0];
                    let reply = output.read_exact(&mut byte).map(|_| byte[0]);
                    let failed = reply.is_err();
                    if sender.try_send(reply).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(pipe)
    }

    fn resize(&mut self, rows: u16, columns: u16, timeout: Duration) -> io::Result<bool> {
        let input = self.input.as_mut().expect("live resize stdin");
        writeln!(input, "{rows} {columns}")?;
        input.flush()?;
        let reply = self.replies.recv_timeout(timeout).map_err(|error| {
            io::Error::new(
                if matches!(error, mpsc::RecvTimeoutError::Timeout) {
                    io::ErrorKind::TimedOut
                } else {
                    io::ErrorKind::BrokenPipe
                },
                error,
            )
        })??;
        match reply {
            b'1' => Ok(true),
            b'0' => Ok(false),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid resize reply",
            )),
        }
    }
}

impl Drop for ResizePipe {
    fn drop(&mut self) {
        // EOF lets the Linux shell exit too; terminating only wsl.exe can leave
        // the remote shell alive. Wait briefly on this background worker only.
        self.input.take();
        let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(windows)]
pub struct WslResize {
    distro: String,
    tty_path_file: String,
    pipe: Option<ResizePipe>,
    reconnect_after: Option<Instant>,
}

#[cfg(windows)]
impl WslResize {
    pub fn new(distro: String, tty_path_file: String) -> Self {
        Self {
            distro,
            tty_path_file,
            pipe: None,
            reconnect_after: None,
        }
    }

    // Called on the resize worker during terminal startup, never on the UI thread.
    pub fn prepare(&mut self) {
        if self.pipe.is_none() && self.reconnect_after.is_none_or(|at| Instant::now() >= at) {
            let mut command = wsl_command(&self.distro);
            command
                .args(["--exec", "/bin/sh", "-c", RESIZE_SCRIPT, "araseo-resize"])
                .arg(&self.tty_path_file);
            self.pipe = ResizePipe::spawn(&mut command).ok();
            if self.pipe.is_none() {
                self.reconnect_after = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    pub fn resize(&mut self, rows: u16, columns: u16) -> bool {
        self.prepare();
        if let Some(pipe) = self.pipe.as_mut() {
            match pipe.resize(rows, columns, Duration::from_secs(3)) {
                Ok(applied) => return applied,
                Err(_) => {
                    self.pipe.take();
                    self.reconnect_after = Some(Instant::now() + Duration::from_secs(5));
                }
            }
        }
        // Preserve the previous working path if the persistent pipe is broken.
        // Back off reconnects to avoid spawning two WSL processes per request.
        self.resize_once(rows, columns)
    }

    pub fn resize_once(&self, rows: u16, columns: u16) -> bool {
        let mut command = wsl_command(&self.distro);
        command.args(["--exec", "/bin/sh", "-c",
            "IFS= read -r tty < \"$1\" && exec /usr/bin/stty -F \"$tty\" rows \"$2\" cols \"$3\"",
            "araseo-resize"])
            .arg(&self.tty_path_file).arg(rows.to_string()).arg(columns.to_string());
        crate::process_job::output(&mut command, Duration::from_secs(3))
            .is_ok_and(|output| output.status.success())
    }
}

#[cfg(windows)]
fn wsl_command(distro: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
    command.creation_flags(0x0800_0000).args(["-d", distro]);
    command
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn persistent_pipe_resizes_a_real_pty_and_recovers_from_missing_tty() {
        let path = std::env::temp_dir().join(format!(
            "araseo-resize-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut shell = CommandBuilder::new("/bin/sh");
        shell.arg("-c");
        shell.arg("tty > \"$1\"; exec cat");
        shell.arg("araseo-test");
        shell.arg(&path);
        let mut terminal = pty.slave.spawn_command(shell).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", RESIZE_SCRIPT, "araseo-resize"])
            .arg(&path);
        let mut pipe = ResizePipe::spawn(&mut command).unwrap();
        let pid = pipe.child.id();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !path.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        for (rows, cols) in [(40, 120), (30, 90), (24, 80)] {
            assert!(pipe.resize(rows, cols, Duration::from_secs(1)).unwrap());
            let size = pty.master.get_size().unwrap();
            assert_eq!((size.rows, size.cols), (rows, cols));
            assert_eq!(pipe.child.id(), pid, "connection must be reused");
        }
        let tty = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!pipe.resize(50, 140, Duration::from_secs(1)).unwrap());
        fs::write(&path, tty).unwrap();
        assert!(pipe.resize(50, 140, Duration::from_secs(1)).unwrap());
        assert_eq!(pty.master.get_size().unwrap().cols, 140);
        drop(pipe);
        // Drop must close/reap the helper, without touching the terminal process.
        assert!(
            !Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        assert!(terminal.try_wait().unwrap().is_none());
        let _ = terminal.kill();
        let _ = terminal.wait();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn dead_or_unresponsive_helpers_do_not_report_success_or_wait_forever() {
        let mut dead = Command::new("/bin/sh");
        dead.args(["-c", "exit 0"]);
        let mut pipe = ResizePipe::spawn(&mut dead).unwrap();
        assert!(pipe.resize(40, 120, Duration::from_millis(100)).is_err());
        drop(pipe);
        let mut blocked = Command::new("/bin/sh");
        blocked.args(["-c", "while IFS= read -r line; do :; done"]);
        let mut pipe = ResizePipe::spawn(&mut blocked).unwrap();
        let now = Instant::now();
        assert_eq!(
            pipe.resize(40, 120, Duration::from_millis(40))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(now.elapsed() < Duration::from_secs(1));
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    #[ignore = "Requires Windows with a running Ubuntu WSL distribution"]
    fn broken_wsl_pipe_falls_back_and_reconnects_without_touching_terminal_input() {
        let path = format!("/tmp/araseo-resize-recovery-{}", std::process::id());
        let python = "import os,sys\nm,s=os.openpty()\ntry:\n with open(sys.argv[1],'x') as f: f.write(os.ttyname(s)+'\\n')\n print('ready',flush=True)\n sys.stdin.read()\nfinally:\n os.unlink(sys.argv[1])\n";
        let mut command = wsl_command("Ubuntu");
        command.args(["--exec", "python3", "-u", "-c", python, &path]);
        // Reuse production child cleanup to close stdin and reap the fixture.
        let mut fixture = ResizePipe::spawn(&mut command).unwrap();
        // Fixture readiness is the first byte 'r', with a dedicated output reader.
        assert_eq!(
            fixture
                .replies
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap(),
            b'r'
        );
        let mut transport = WslResize::new("Ubuntu".into(), path.clone());
        assert!(transport.resize(40, 120));
        let pipe = transport.pipe.as_mut().unwrap();
        pipe.child.kill().unwrap();
        pipe.child.wait().unwrap();
        assert!(
            transport.resize(30, 90),
            "broken pipe must fall back to stty"
        );
        assert!(transport.pipe.is_none());
        let backoff = transport.reconnect_after.unwrap();
        assert!(transport.resize(24, 80));
        assert_eq!(
            transport.reconnect_after,
            Some(backoff),
            "fallback must not extend reconnect cooldown"
        );
        transport.reconnect_after = Some(Instant::now());
        assert!(transport.resize(50, 140));
        assert!(transport.pipe.is_some());
        let mut verify = wsl_command("Ubuntu");
        verify.args([
            "--exec",
            "/bin/sh",
            "-c",
            "IFS= read -r tty < \"$1\"; stty -F \"$tty\" size",
            "verify",
            &path,
        ]);
        let out = verify.output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "50 140");
        drop(transport);
        assert!(fixture.child.try_wait().unwrap().is_none());
        drop(fixture);
    }
}
