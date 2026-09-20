//! Read-only, on-demand checks for the WSL workspace used by the desktop UI.
use crate::workspace::Workspace;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const PROBE: &str = r#"
printf 'KERNEL='; uname -r
if [ -d "$1" ]; then echo 'WORKSPACE=ok'; else echo 'WORKSPACE=missing'; fi
printf 'MEM_KIB='; awk '/^MemAvailable:/ { print $2 }' /proc/meminfo
printf 'GIT='; if command -v git >/dev/null 2>&1; then echo yes; else echo no; fi
printf 'DOCKER_DAEMON='; if ps -eo comm= 2>/dev/null | grep -qx dockerd; then echo yes; else echo no; fi
ps -eo pid=,rss=,etime=,comm= --sort=-rss 2>/dev/null | head -n 8 | sed 's/^/PROCESS=/'
"#;

const HOST_DISK_PROBE: &str = r#"
$ErrorActionPreference = 'Stop'
$entry = Get-ChildItem 'Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Lxss' |
    Get-ItemProperty | Where-Object { $_.DistributionName -eq $distro } | Select-Object -First 1
if (-not $entry) { exit 2 }
$vhd = Get-ChildItem -LiteralPath $entry.BasePath -Filter '*.vhdx' | Select-Object -First 1
if (-not $vhd) { exit 3 }
$drive = New-Object System.IO.DriveInfo ([System.IO.Path]::GetPathRoot($vhd.FullName))
Write-Output ('HOST_DRIVE=' + $drive.Name)
Write-Output ('HOST_FREE_BYTES=' + $drive.AvailableFreeSpace)
Write-Output ('HOST_TOTAL_BYTES=' + $drive.TotalSize)
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDisk {
    pub drive: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub rss_kib: u64,
    pub elapsed: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockerUsage {
    pub name: String,
    pub cpu: String,
    pub memory: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    pub running_distros: Option<Vec<String>>,
    pub processes: Vec<ProcessInfo>,
    pub docker_containers: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Good,
    Warning,
    Error,
    Info,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Info => "info",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub title: String,
    pub level: Level,
    pub detail: String,
    pub kind: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
    pub docker_usage_available: bool,
}

impl Report {
    fn add(&mut self, title: impl Into<String>, level: Level, detail: impl Into<String>) {
        self.checks.push(Check {
            title: title.into(),
            level,
            detail: detail.into(),
            kind: "check",
        });
    }

    fn add_process(&mut self, process: &ProcessInfo) {
        self.checks.push(Check {
            title: format!("{} · PID {}", process.name, process.pid),
            level: Level::Info,
            detail: format!(
                "{:.1} MiB memory · running {}",
                process.rss_kib as f64 / 1024.0,
                process.elapsed
            ),
            kind: "process",
        });
    }

    pub fn copy_text(&self) -> String {
        let mut result = String::from("Araseo WSL environment diagnostics\n");
        for check in &self.checks {
            result.push_str(&format!(
                "[{}] {}: {}\n",
                check.level.as_str(),
                check.title,
                check.detail
            ));
        }
        result
    }
}

/// Runs off the UI thread. Nothing is changed in Windows or the distribution.
pub fn collect(workspace: &Workspace) -> Report {
    let probe = run_probe(workspace);
    let host_accessible = probe.is_ok() && workspace.host_root.is_dir();
    let host_disk = run_host_disk_probe(&workspace.distro).ok();
    let processes = probe
        .as_ref()
        .ok()
        .map(|output| parse_processes(output))
        .unwrap_or_default();
    let running_distros = run_running_distros().ok();
    let docker_detected = docker_engine_running(
        running_distros.as_deref(),
        probe.as_ref().ok().map(String::as_str),
    );
    let docker_containers = docker_detected
        .then(|| run_docker_command(workspace, &["ps", "--format", "{{.Names}}"]).ok())
        .flatten()
        .map(|output| {
            output
                .lines()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect()
        });
    let activity = Activity {
        running_distros,
        processes,
        docker_containers,
    };
    evaluate(
        workspace,
        host_accessible,
        probe.as_ref().ok().map(String::as_str),
        host_disk.as_ref(),
        &activity,
    )
}

pub fn collect_docker_usage(workspace: &Workspace) -> io::Result<Vec<DockerUsage>> {
    let output = run_docker_command(
        workspace,
        &[
            "stats",
            "--no-stream",
            "--format",
            "{{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}",
        ],
    )?;
    Ok(parse_docker_usage(&output))
}

fn parse_docker_usage(output: &str) -> Vec<DockerUsage> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            Some(DockerUsage {
                name: fields.next()?.to_owned(),
                cpu: fields.next()?.to_owned(),
                memory: fields.next()?.to_owned(),
            })
        })
        .collect()
}

fn docker_desktop_running(running_distros: Option<&[String]>) -> bool {
    running_distros.is_some_and(|names| {
        names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("docker-desktop"))
    })
}

fn docker_engine_running(running_distros: Option<&[String]>, probe: Option<&str>) -> bool {
    docker_desktop_running(running_distros)
        || probe.is_some_and(|output| value(output, "DOCKER_DAEMON=") == Some("yes"))
}

fn run_probe(workspace: &Workspace) -> io::Result<String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
        command.args(["--distribution", &workspace.distro, "--exec", "sh"]);
        command
    } else {
        Command::new("sh")
    };
    command.args(["-c", PROBE, "sh"]);
    command.arg(&workspace.linux_root);
    hide_window(&mut command);
    run_command(command)
}

fn run_host_disk_probe(distro: &str) -> io::Result<HostDisk> {
    let executable = if cfg!(windows) {
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
    } else {
        "powershell.exe"
    };
    let mut command = Command::new(executable);
    // PowerShell single-quoted strings escape an apostrophe by doubling it.
    let quoted_distro = distro.replace('\'', "''");
    let script = format!("$distro = '{quoted_distro}'\n{HOST_DISK_PROBE}");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    hide_window(&mut command);
    let output = run_command(command)?;
    let drive = value(&output, "HOST_DRIVE=")
        .filter(|drive| !drive.is_empty())
        .ok_or_else(|| io::Error::other("Windows host drive was not reported"))?
        .to_owned();
    let free_bytes = value(&output, "HOST_FREE_BYTES=")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::other("Windows host free space was not reported"))?;
    let total_bytes = value(&output, "HOST_TOTAL_BYTES=")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::other("Windows host capacity was not reported"))?;
    Ok(HostDisk {
        drive,
        free_bytes,
        total_bytes,
    })
}

fn run_running_distros() -> io::Result<Vec<String>> {
    let executable = if cfg!(windows) {
        r"C:\Windows\System32\wsl.exe"
    } else {
        "wsl.exe"
    };
    let mut command = Command::new(executable);
    command.args(["--list", "--running", "--quiet"]);
    hide_window(&mut command);
    Ok(parse_running_distros(&run_command_bytes(command)?))
}

fn parse_running_distros(bytes: &[u8]) -> Vec<String> {
    let text =
        if bytes.len() >= 2 && bytes.iter().filter(|byte| **byte == 0).count() > bytes.len() / 4 {
            String::from_utf16_lossy(
                &bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect::<Vec<_>>(),
            )
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        };
    text.lines()
        .map(|line| line.trim_matches(['\u{feff}', '\r', '\0', ' ', '\t']))
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_processes(output: &str) -> Vec<ProcessInfo> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.strip_prefix("PROCESS=")?.split_whitespace();
            Some(ProcessInfo {
                pid: fields.next()?.parse().ok()?,
                rss_kib: fields.next()?.parse().ok()?,
                elapsed: fields.next()?.to_owned(),
                name: fields.next()?.to_owned(),
            })
        })
        .collect()
}

fn run_docker_command(workspace: &Workspace, args: &[&str]) -> io::Result<String> {
    let windows_cli = if cfg!(windows) {
        r"C:\Program Files\Docker\Docker\resources\bin\docker.exe"
    } else {
        "/mnt/c/Program Files/Docker/Docker/resources/bin/docker.exe"
    };
    if Path::new(windows_cli).is_file() {
        let mut command = Command::new(windows_cli);
        command.args(args);
        hide_window(&mut command);
        if let Ok(output) = run_command(command) {
            return Ok(output);
        }
    }
    let mut command = if cfg!(windows) {
        let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
        command.args(["--distribution", &workspace.distro, "--exec", "docker"]);
        command
    } else {
        Command::new("docker")
    };
    command.args(args);
    hide_window(&mut command);
    run_command(command)
}

fn run_command(command: Command) -> io::Result<String> {
    Ok(String::from_utf8_lossy(&run_command_bytes(command)?).into_owned())
}

fn run_command_bytes(mut command: Command) -> io::Result<Vec<u8>> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            if output.status.success() {
                return Ok(output.stdout);
            }
            return Err(io::Error::other(format!(
                "probe exited with {}",
                output.status
            )));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "diagnostic command did not respond within 8 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "windows")]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(target_os = "windows"))]
fn hide_window(_command: &mut Command) {}

fn value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .map(str::trim)
}

fn windows_mount(path: &Path) -> bool {
    let mut parts = path.components();
    let _ = parts.next(); // root
    matches!(parts.next(), Some(std::path::Component::Normal(mnt)) if mnt == "mnt")
        && matches!(parts.next(), Some(std::path::Component::Normal(drive)) if drive.to_string_lossy().len() == 1 && drive.to_string_lossy().chars().all(|c| c.is_ascii_alphabetic()))
}

/// Pure assessment logic shared with the headless Harness.
pub fn evaluate(
    workspace: &Workspace,
    host_accessible: bool,
    probe: Option<&str>,
    host_disk: Option<&HostDisk>,
    activity: &Activity,
) -> Report {
    let mut report = Report::default();
    match probe {
        Some(output) if value(output, "KERNEL=").is_some_and(|v| !v.is_empty()) => {
            report.add(
                "WSL connection",
                Level::Good,
                format!(
                    "{} · kernel {}",
                    workspace.distro,
                    value(output, "KERNEL=").unwrap()
                ),
            );
        }
        _ => report.add(
            "WSL connection",
            Level::Error,
            format!(
                "Could not query {}. Check that the distribution is running, then retry.",
                workspace.distro
            ),
        ),
    }
    match probe.and_then(|output| value(output, "WORKSPACE=")) {
        Some("ok") if host_accessible => report.add(
            "Workspace access",
            Level::Good,
            "Accessible from WSL and Windows",
        ),
        Some("ok") => report.add(
            "Workspace access",
            Level::Error,
            "WSL can see the folder, but Windows cannot. Check \\\\wsl.localhost access.",
        ),
        Some("missing") => report.add(
            "Workspace access",
            Level::Error,
            "Project folder is missing in WSL.",
        ),
        _ => report.add(
            "Workspace access",
            Level::Error,
            "Could not verify the project folder.",
        ),
    }
    if windows_mount(&workspace.linux_root) {
        report.add("Project location", Level::Warning, format!("{} is on a Windows-mounted drive. Linux builds and Git are usually faster under /home.", workspace.linux_root.display()));
    } else if workspace.linux_root.starts_with("/home") {
        report.add(
            "Project location",
            Level::Good,
            format!(
                "{} is under /home, the usual WSL project location.",
                workspace.linux_root.display()
            ),
        );
    } else {
        report.add(
            "Project location",
            Level::Info,
            format!("{} is outside common Windows drive mounts; filesystem performance was not measured.", workspace.linux_root.display()),
        );
    }
    match host_disk {
        Some(disk) if disk.total_bytes > 0 => {
            let level = if disk.free_bytes < 10 * 1024 * 1024 * 1024
                || (disk.free_bytes as u128) * 10 < disk.total_bytes as u128
            {
                Level::Warning
            } else {
                Level::Good
            };
            report.add(
                "Windows host disk",
                level,
                format!(
                    "{}: {:.1} GiB free of {:.1} GiB; this drive stores the WSL VHDX.",
                    disk.drive,
                    disk.free_bytes as f64 / 1024.0_f64.powi(3),
                    disk.total_bytes as f64 / 1024.0_f64.powi(3)
                ),
            );
        }
        _ => report.add(
            "Windows host disk",
            Level::Info,
            "Could not locate the drive storing this distribution's WSL VHDX.",
        ),
    }
    let memory = probe
        .and_then(|output| value(output, "MEM_KIB="))
        .and_then(|v| v.parse::<u64>().ok());
    match memory {
        Some(kib) if kib < 1024 * 1024 => report.add(
            "Available memory",
            Level::Warning,
            format!(
                "{:.1} GiB available in WSL right now; large builds may be constrained.",
                kib as f64 / 1024.0 / 1024.0
            ),
        ),
        Some(kib) => report.add(
            "Available memory",
            Level::Good,
            format!(
                "{:.1} GiB available in WSL right now",
                kib as f64 / 1024.0 / 1024.0
            ),
        ),
        None => report.add(
            "Available memory",
            Level::Info,
            "Memory availability could not be read.",
        ),
    }
    match &activity.running_distros {
        Some(names) if !names.is_empty() => report.add(
            "Running WSL distributions",
            Level::Info,
            format!("{} running: {}", names.len(), names.join(", ")),
        ),
        Some(_) => report.add(
            "Running WSL distributions",
            Level::Info,
            "None reported as running.",
        ),
        None => report.add(
            "Running WSL distributions",
            Level::Info,
            "Could not list running distributions.",
        ),
    }
    if activity.processes.is_empty() {
        report.add(
            "Top processes",
            Level::Info,
            "Could not read processes in this distribution.",
        );
    } else {
        report.add(
            "Top processes",
            Level::Info,
            format!(
                "{} highest by resident memory in {}. Activity is a snapshot, not a usage verdict.",
                activity.processes.len(),
                workspace.distro
            ),
        );
        for process in &activity.processes {
            report.add_process(process);
        }
    }
    let docker_desktop_running = docker_desktop_running(activity.running_distros.as_deref());
    let native_docker_running =
        probe.is_some_and(|output| value(output, "DOCKER_DAEMON=") == Some("yes"));
    let docker_label = if docker_desktop_running {
        "Docker Desktop"
    } else {
        "Docker Engine"
    };
    match &activity.docker_containers {
        Some(names) if names.is_empty() => report.add("Docker", Level::Warning, format!("{docker_label} is running with 0 containers. It may be an unused background service.")),
        Some(names) => {
            report.docker_usage_available = true;
            report.add("Docker", Level::Info, format!("{docker_label} is running with {} containers: {}", names.len(), names.join(", ")));
        }
        None if docker_desktop_running || native_docker_running => report.add("Docker", Level::Info, format!("{docker_label} is running; container count is unavailable from this distribution.")),
        None => report.add("Docker", Level::Good, "No Docker engine detected in running WSL distributions."),
    }
    match probe.and_then(|output| value(output, "GIT=")) {
        Some("yes") => report.add("Git", Level::Good, "Git is available in this distribution."),
        Some("no") => report.add(
            "Git",
            Level::Warning,
            "Git is not installed in this distribution.",
        ),
        _ => report.add("Git", Level::Info, "Git availability could not be checked."),
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace(path: &str) -> Workspace {
        Workspace::prepare("Ubuntu", PathBuf::from(path)).unwrap()
    }

    #[test]
    fn warns_for_windows_mount_and_low_resources() {
        let report = evaluate(
            &workspace("/mnt/c/work"),
            true,
            Some("KERNEL=6.6\nWORKSPACE=ok\nMEM_KIB=500000\nGIT=no\n"),
            None,
            &Activity::default(),
        );
        assert_eq!(report.checks[2].level, Level::Warning);
        assert_eq!(report.checks[4].level, Level::Warning);
        assert!(report.copy_text().contains("/mnt/c/work"));
        assert!(!report.copy_text().contains("virtual disk"));
    }

    #[test]
    fn reports_independent_connection_and_host_access_failures() {
        let report = evaluate(
            &workspace("/home/me/project"),
            false,
            Some("KERNEL=6.6\nWORKSPACE=ok\nMEM_KIB=4000000\nGIT=yes\n"),
            None,
            &Activity::default(),
        );
        assert_eq!(report.checks[0].level, Level::Good);
        assert_eq!(report.checks[1].level, Level::Error);
        assert_eq!(report.checks[2].level, Level::Good);
        assert_eq!(
            evaluate(
                &workspace("/home/me/project"),
                false,
                None,
                None,
                &Activity::default()
            )
            .checks[0]
                .level,
            Level::Error
        );
    }

    #[test]
    fn reports_physical_host_space_without_a_virtual_disk_row() {
        let host = HostDisk {
            drive: "C:\\".into(),
            free_bytes: 23 * 1024 * 1024 * 1024,
            total_bytes: 238 * 1024 * 1024 * 1024,
        };
        let report = evaluate(
            &workspace("/home/me/project"),
            true,
            Some("KERNEL=6.6-microsoft-standard-WSL2\nWORKSPACE=ok\nMEM_KIB=4000000\nGIT=yes\n"),
            Some(&host),
            &Activity::default(),
        );
        assert_eq!(report.checks[3].title, "Windows host disk");
        assert_eq!(report.checks[3].level, Level::Warning);
        assert!(report.checks[3].detail.contains("23.0 GiB free"));
        assert!(!report.copy_text().contains("WSL virtual disk"));

        let unknown = evaluate(
            &workspace("/home/me/project"),
            true,
            None,
            None,
            &Activity::default(),
        );
        assert_eq!(unknown.checks[3].level, Level::Info);
    }

    #[test]
    fn running_distros_and_docker_are_visible_without_starting_a_stopped_distro() {
        let utf16 = "Ubuntu\r\ndocker-desktop\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(parse_running_distros(&utf16), ["Ubuntu", "docker-desktop"]);
        assert!(!docker_engine_running(
            Some(&["Ubuntu".into()]),
            Some("DOCKER_DAEMON=no\n")
        ));
        assert!(docker_engine_running(
            Some(&["Ubuntu".into(), "docker-desktop".into()]),
            None
        ));
        assert_eq!(
            parse_docker_usage("web\t0.03%\t22MiB / 4GiB\n"),
            vec![DockerUsage {
                name: "web".into(),
                cpu: "0.03%".into(),
                memory: "22MiB / 4GiB".into()
            }]
        );
        let activity = Activity {
            running_distros: Some(vec!["Ubuntu".into(), "docker-desktop".into()]),
            processes: parse_processes(
                "PROCESS=42 227424 04:48:39 codex\nPROCESS=175 12376 1-20:31:51 ollama\n",
            ),
            docker_containers: Some(Vec::new()),
        };
        let report = evaluate(
            &workspace("/home/me/project"),
            true,
            Some("KERNEL=6.6\nWORKSPACE=ok\nMEM_KIB=4000000\nGIT=yes\n"),
            None,
            &activity,
        );
        assert!(
            report
                .copy_text()
                .contains("2 running: Ubuntu, docker-desktop")
        );
        assert!(report.copy_text().contains("codex · PID 42"));
        assert!(
            report
                .copy_text()
                .contains("Docker Desktop is running with 0 containers")
        );
        assert!(!report.docker_usage_available);
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.title == "Docker")
                .unwrap()
                .level,
            Level::Warning
        );

        let mut active = activity;
        active.docker_containers = Some(vec!["web".into(), "database".into()]);
        let report = evaluate(&workspace("/home/me/project"), true, None, None, &active);
        assert!(report.docker_usage_available);
        assert!(report.copy_text().contains("2 containers: web, database"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn collects_a_real_read_only_shell_probe() {
        let root = std::env::current_dir().unwrap();
        let distro = std::env::var("WSL_DISTRO_NAME").unwrap_or_else(|_| "Ubuntu".into());
        let workspace = Workspace::new(&distro, root).unwrap();
        let report = collect(&workspace);
        assert_eq!(report.checks[0].level, Level::Good);
        assert_eq!(report.checks[1].level, Level::Good);
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.title == "Top processes" && check.detail.contains("highest"))
        );
        assert!(!report.copy_text().contains("WSL virtual disk"));
        if std::env::var_os("WSL_DISTRO_NAME").is_some() {
            let host = report
                .checks
                .iter()
                .find(|check| check.title == "Windows host disk")
                .unwrap();
            assert!(host.detail.contains("GiB free"), "{}", host.detail);
        }
    }
}
