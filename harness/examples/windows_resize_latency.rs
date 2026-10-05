//! Run on Windows against a disposable WSL PTY; no existing terminal is touched.
#[cfg(windows)]
fn main() {
    use araseo_harness::terminal_resize::WslResize;
    use std::io::{BufRead, BufReader};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    struct Fixture(std::process::Child);
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.0.stdin.take();
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let distro = std::env::args().nth(1).unwrap_or_else(|| "Ubuntu".into());
    let path = format!("/tmp/araseo-resize-benchmark-{}", std::process::id());
    let python = r#"import os,sys,signal
master,slave=os.openpty()
pid=os.fork()
if pid==0:
    os.setsid()
    import fcntl,termios
    fcntl.ioctl(slave,termios.TIOCSCTTY,0)
    signal.signal(signal.SIGWINCH,lambda *_: print('winch',flush=True))
    print('child-ready',flush=True)
    while True: signal.pause()
else:
    try:
        with open(sys.argv[1],'x') as f: f.write(os.ttyname(slave)+'\n')
        print('ready',flush=True)
        sys.stdin.read()
    finally:
        os.kill(pid,signal.SIGTERM)
        os.waitpid(pid,0)
        os.unlink(sys.argv[1])
"#;
    let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
    command
        .creation_flags(0x0800_0000)
        .args([
            "-d", &distro, "--exec", "python3", "-u", "-c", python, &path,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut fixture = Fixture(command.spawn().expect("start WSL fixture"));
    let output = fixture.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if let Ok(line) = line {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ready = 0;
    while ready < 2 {
        let line = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("PTY startup");
        if line == "ready" || line == "child-ready" {
            ready += 1;
        }
    }
    let mut transport = WslResize::new(distro.clone(), path.clone());
    let now = Instant::now();
    assert!(transport.resize(30, 90));
    println!(
        "Persistent first request: {:.1} ms",
        now.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "winch");
    let mut old = Vec::new();
    let mut new = Vec::new();
    for i in 0..12 {
        let now = Instant::now();
        assert!(transport.resize_once(40 + i, 120 + i));
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "winch");
        old.push(now.elapsed().as_secs_f64() * 1000.0);
        let now = Instant::now();
        assert!(transport.resize(30 + i, 90 + i));
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "winch");
        new.push(now.elapsed().as_secs_f64() * 1000.0);
    }
    let mut verify = Command::new(r"C:\Windows\System32\wsl.exe");
    verify.creation_flags(0x0800_0000).args([
        "-d",
        &distro,
        "--exec",
        "/bin/sh",
        "-c",
        "IFS= read -r tty < \"$1\"; stty -F \"$tty\" size",
        "verify",
        &path,
    ]);
    let out = verify.output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "41 101");
    old.sort_by(f64::total_cmp);
    new.sort_by(f64::total_cmp);
    println!(
        "Legacy request + SIGWINCH median: {:.1} ms (12 requests)",
        old[6]
    );
    println!(
        "Persistent request + SIGWINCH median: {:.1} ms (12 requests)",
        new[6]
    );
    println!("Actual Linux PTY size and foreground SIGWINCH verified for every request.");
    drop(transport);
    println!(
        "Resize helper closed; fixture still running: {}",
        fixture.0.try_wait().unwrap().is_none()
    );
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Run this fixture on Windows.");
}
