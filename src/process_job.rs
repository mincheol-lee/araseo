//! Run external tools on workers with a deadline, without stranded pipe-reader threads.
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(0);
struct Capture {
    file: File,
    path: PathBuf,
}
impl Capture {
    fn new() -> io::Result<Self> {
        for _ in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "araseo-tool-{}-{}",
                std::process::id(),
                NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => return Ok(Self { file, path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create command capture",
        ))
    }
    fn read(&mut self) -> io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
pub fn output(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let mut stdout = Capture::new()?;
    let mut stderr = Capture::new()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.file.try_clone()?))
        .stderr(Stdio::from(stderr.file.try_clone()?));
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "external command timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Ok(Output {
            status,
            stdout: stdout.read()?,
            stderr: stderr.read()?,
        })
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    // Drop the command's cloned capture handles before deleting the files.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    result
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn captures_both_streams_and_times_out_without_waiting_for_completion() {
        let result = output(
            Command::new("sh").args(["-c", "printf out; printf err >&2"]),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(result.stdout, b"out");
        assert_eq!(result.stderr, b"err");
        let now = Instant::now();
        let error = output(
            Command::new("sh").args(["-c", "exec sleep 10"]),
            Duration::from_millis(80),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(now.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn descendant_holding_output_handles_cannot_hold_up_completion() {
        let now = Instant::now();
        let result = output(
            Command::new("sh").args(["-c", "sleep 0.3 & printf done"]),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(result.stdout, b"done");
        assert!(now.elapsed() < Duration::from_millis(250));
    }
}
