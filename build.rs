fn main() {
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into());
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .is_some_and(|output| !output.stdout.is_empty());
    println!(
        "cargo:rustc-env=ARASEO_BUILD_REVISION={revision}{}",
        if dirty { "+dirty" } else { "" }
    );
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=Cargo.toml");
    // The generated UI needs more stack than Windows gives a process main thread
    // in release builds. Compile it on a thread with an explicit stack size.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| slint_build::compile("ui/app.slint"))
        .expect("failed to start Slint UI compilation")
        .join()
        .expect("Slint UI compilation panicked")
        .expect("failed to compile Slint UI");
}
