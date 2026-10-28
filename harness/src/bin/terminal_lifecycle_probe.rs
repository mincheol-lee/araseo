#![cfg_attr(windows, windows_subsystem = "windows")]
//! Native Windows regression driver using production terminal lifetime code.
use std::{
    io::{self, BufRead, Write},
    path::Path,
    time::{Duration, Instant},
};
fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    araseo_harness::terminal_host::run_if_requested();
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(
        args.len() == 3,
        "usage: terminal_lifecycle_probe DISTRO ROOT"
    );
    let mut terminal = Some(araseo_harness::terminal::TerminalSession::spawn(
        &args[1],
        Path::new(&args[2]),
    )?);
    println!("READY");
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line == "drop" {
            drop(terminal.take());
            println!("DROPPED");
        } else if let Some(command) = line.strip_prefix("input ") {
            terminal
                .as_mut()
                .unwrap()
                .write(format!("{command}\r").as_bytes());
            println!("SENT");
        } else if line == "resize" {
            terminal.as_mut().unwrap().resize(40, 120);
            let deadline = Instant::now() + Duration::from_millis(300);
            while Instant::now() < deadline {
                terminal.as_mut().unwrap().poll();
                std::thread::sleep(Duration::from_millis(5));
            }
            println!("RESIZED");
        } else if line == "exit" {
            break;
        } else {
            anyhow::bail!("unknown driver command");
        }
        io::stdout().flush()?;
    }
    Ok(())
}
