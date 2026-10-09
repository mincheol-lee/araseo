//! Controlled Codex 0.160.1 input probe; no prompt is submitted.
use araseo_harness::codex_fixture::text;
use araseo_harness::terminal::TerminalSession;
use std::{
    path::Path,
    time::{Duration, Instant},
};
fn wait(terminal: &mut TerminalSession, needle: &str, dir: &Path, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        terminal.poll();
        let output = text(terminal);
        if output.contains(needle) {
            std::fs::write(dir.join(name), output).unwrap();
            return;
        }
        if Instant::now() >= deadline {
            std::fs::write(dir.join(name), &output).unwrap();
            panic!(
                "did not see {needle}; capture: {}",
                dir.join(name).display()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn main() {
    let (mut terminal, host_root) = araseo_harness::codex_fixture::spawn("probe");
    wait(&mut terminal, "for shortcuts", &host_root, "ready.txt");
    terminal.write(b"beforemarker");
    wait(&mut terminal, "beforemarker", &host_root, "before.txt");
    terminal.resize(20, 120);
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        terminal.resize(20, 120);
        terminal.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    terminal.write(b"aftermarker");
    wait(
        &mut terminal,
        "beforemarkeraftermarker",
        &host_root,
        "after.txt",
    );
    println!(
        "Actual Codex input before/after PTY resize passed; captures: {}",
        host_root.display()
    );
    // No Enter is sent, and the configured provider points to a closed loopback port.
}
