#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use anyhow::{Context, Result, bail};
use araseo::{diagnostics_log, gpui_app, workspace};
use std::path::PathBuf;

fn main() -> Result<()> {
    #[cfg(windows)]
    araseo::terminal_host::run_if_requested();
    diagnostics_log::init();
    let result = (|| {
        let (distro, root, initial) = parse_args()?;
        let workspace = workspace::Workspace::prepare(distro, root)?;
        gpui_app::run(workspace, initial)
    })();
    match &result {
        Ok(()) => diagnostics_log::event("normal exit"),
        Err(error) => diagnostics_log::event(&format!("application error: {error:#}")),
    };
    result
}
fn parse_args() -> Result<(String, PathBuf, Option<PathBuf>)> {
    let mut args = std::env::args().skip(1);
    let mut distro = std::env::var("WSL_DISTRO_NAME").unwrap_or_else(|_| "Ubuntu".into());
    let mut root: Option<PathBuf> = None;
    let mut file = None;
    let mut internal_root = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--distro" => distro = args.next().context("--distro requires a value")?,
            "--root" => {
                root = Some(PathBuf::from(
                    args.next().context("--root requires a value")?,
                ));
                internal_root = true;
            }
            "--file" => {
                file = Some(PathBuf::from(
                    args.next().context("--file requires a value")?,
                ))
            }
            "--help" | "-h" => {
                println!(
                    "Araseo {}\nUsage: araseo [PATH]\n       araseo.exe --distro NAME --root LINUX_PATH [--file FILE]",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("araseo {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            value if value.starts_with('-') => bail!("unknown option: {value}"),
            value => root = Some(PathBuf::from(value)),
        }
    }
    let supplied = root.unwrap_or(std::env::current_dir()?);
    let supplied = if internal_root {
        supplied
    } else {
        supplied
            .canonicalize()
            .with_context(|| format!("cannot resolve path: {}", supplied.display()))?
    };
    let root = if supplied.is_file() {
        file = Some(supplied.clone());
        supplied
            .parent()
            .context("file does not have a parent directory")?
            .to_path_buf()
    } else {
        supplied
    };
    Ok((distro, root, file))
}
