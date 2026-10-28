//! One-shot orphan recovery and Linux tree cleanup. The ordinary terminal I/O
//! path does not pass through this module or through Python.
#[cfg(any(windows, test))]
pub const CLEANUP_SCRIPT: &str = include_str!("terminal_cleanup.py");
#[cfg(windows)]
use std::{
    collections::HashSet,
    process::Command,
    sync::{Mutex, OnceLock},
    time::Duration,
};
#[cfg(windows)]
fn cleanup_command(distro: &str, mode: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
    command.creation_flags(0x08000000).args([
        "-d",
        distro,
        "--exec",
        "/usr/bin/setpriv",
        "--pdeathsig",
        "KILL",
        "python3",
        "-c",
        CLEANUP_SCRIPT,
        mode,
    ]);
    command
}
#[cfg(windows)]
pub fn close(distro: &str, token: &str) {
    let _ = crate::process_job::output(
        cleanup_command(distro, "close").arg(token),
        Duration::from_secs(6),
    );
    // Missing Python/pidfd support still closes the PTY through the host's job
    // and setpriv parent-death signal; core terminal functionality needs neither.
}
#[cfg(windows)]
pub fn recover(distro: &str) {
    static CHECKED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    if !CHECKED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(distro.to_owned())
    {
        return;
    }
    let Ok(output) =
        crate::process_job::output(&mut cleanup_command(distro, "list"), Duration::from_secs(4))
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    #[derive(serde::Deserialize, serde::Serialize)]
    struct Candidate {
        token: Option<String>,
        pid: u32,
        start: u64,
        owner_pid: u32,
        owner_created: Option<u64>,
    }
    let Ok(mut candidates) = serde_json::from_slice::<Vec<Candidate>>(&output.stdout) else {
        return;
    };
    candidates
        .retain(|item| !crate::terminal_host::owner_running(item.owner_pid, item.owner_created));
    if !candidates.is_empty() {
        crate::diagnostics_log::event(&format!(
            "recovering {} abandoned WSL terminal groups in {}",
            candidates.len(),
            distro
        ));
    }
    for batch in candidates.chunks(32) {
        if let Ok(payload) = serde_json::to_string(batch) {
            let _ = crate::process_job::output(
                cleanup_command(distro, "recover").arg(payload),
                Duration::from_secs(6),
            );
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
        time::Duration,
    };
    struct Fixture {
        child: std::process::Child,
        token: String,
        directory: std::path::PathBuf,
    }
    impl Fixture {
        fn new(id: u32, stubborn: bool) -> Self {
            let token = format!("987654321.123456.{id}");
            let directory =
                std::env::temp_dir().join(format!("araseo-cleanup-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            let code = r#"import os,pathlib,signal,subprocess,sys,time
root=pathlib.Path(sys.argv[1])
# Detached descendants remain owned through the inherited terminal token.
child=subprocess.Popen([sys.executable,'-c','import signal,time; signal.signal(signal.SIGHUP,signal.SIG_IGN); signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(300)'],start_new_session=True)
(root/'child').write_text(str(child.pid))
def stop(sig,frame):
 (root/'hup').write_text('received')
 if sys.argv[2]=='False':
  try:child.wait(timeout=3)
  except subprocess.TimeoutExpired:pass
  raise SystemExit
signal.signal(signal.SIGHUP,stop)
signal.signal(signal.SIGTERM,signal.SIG_IGN)
print('ready',flush=True)
while True:time.sleep(1)
"#;
            let mut child = Command::new("python3")
                .args(["-u", "-c", code])
                .arg(&directory)
                .arg(
                    stubborn
                        .to_string()
                        .replace("true", "True")
                        .replace("false", "False"),
                )
                .env("ARASEO_TERMINAL_SESSION", &token)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut ready = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut ready)
                .unwrap();
            assert_eq!(ready.trim(), "ready");
            Self {
                child,
                token,
                directory,
            }
        }
        fn close(&self) {
            let output = crate::process_job::output(
                Command::new("python3").args(["-c", CLEANUP_SCRIPT, "close", &self.token]),
                Duration::from_secs(8),
            )
            .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = Command::new("python3")
                .args(["-c", CLEANUP_SCRIPT, "close", &self.token])
                .status();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    #[test]
    fn cleanup_hangs_up_owned_processes_then_stops_detached_stubborn_children_and_keeps_other_sessions()
     {
        let mut owned = Fixture::new(1, true);
        let mut peer = Fixture::new(2, false);
        owned.close();
        assert!(owned.child.try_wait().unwrap().is_some());
        assert!(owned.directory.join("hup").exists());
        assert!(
            peer.child.try_wait().unwrap().is_none(),
            "a different terminal must remain alive"
        );
        let output = Command::new("python3")
            .args(["-c", CLEANUP_SCRIPT, "list"])
            .output()
            .unwrap();
        let candidates: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            !candidates
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["token"] == owned.token)
        );
        assert!(
            candidates
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["token"] == peer.token)
        );
        peer.close();
    }
    #[test]
    fn a_stale_pid_record_cannot_kill_a_different_process() {
        let mut peer = Fixture::new(3, false);
        let payload = format!(r#"[{{"token":null,"pid":{},"start":0}}]"#, peer.child.id());
        let output = Command::new("python3")
            .args(["-c", CLEANUP_SCRIPT, "recover", &payload])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(peer.child.try_wait().unwrap().is_none());
        peer.close();
    }
}
