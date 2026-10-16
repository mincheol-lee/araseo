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
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=assets/araseo.rc");
    println!("cargo:rerun-if-changed=assets/araseo.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // GPUI loads resource 1 for the native window and taskbar icon.
        embed_resource::compile_for(
            "assets/araseo.rc",
            ["araseo"],
            embed_resource::ParamsIncludeDirs(["assets"]),
        )
        .manifest_required()
        .expect("failed to embed the Araseo Windows icon");
    }
}
