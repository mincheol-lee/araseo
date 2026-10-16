//! Compile release shaders for the target, including Windows builds from WSL.
use std::{env, fs, path::{Path, PathBuf}, process::Command};

fn windows_path(path: &Path) -> String {
    if cfg!(windows) { return path.to_string_lossy().into_owned(); }
    let output = Command::new("wslpath").arg("-w").arg(path).output()
        .expect("Windows shader compilation from Linux requires WSL interop");
    assert!(output.status.success(), "could not convert shader path");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn compiler() -> PathBuf {
    if let Some(path) = env::var_os("GPUI_FXC_PATH") { return path.into(); }
    let sdk = if cfg!(windows) {
        PathBuf::from(env::var_os("ProgramFiles(x86)").expect("ProgramFiles(x86)"))
            .join("Windows Kits/10/bin")
    } else { PathBuf::from("/mnt/c/Program Files (x86)/Windows Kits/10/bin") };
    let mut candidates: Vec<_> = fs::read_dir(sdk).expect("install Windows SDK or set GPUI_FXC_PATH")
        .flatten().map(|entry| entry.path().join("x64/fxc.exe"))
        .filter(|path| path.is_file()).collect();
    candidates.sort();
    candidates.pop().expect("Windows SDK fxc.exe is required")
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/shaders.hlsl");
    println!("cargo:rerun-if-changed=src/color_text_raster.hlsl");
    println!("cargo:rerun-if-env-changed=GPUI_FXC_PATH");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("PROFILE").as_deref() != Ok("release") { return; }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let fxc = compiler();
    let mut bindings = String::new();
    for module in ["quad", "shadow", "path_rasterization", "path_sprite", "underline",
        "monochrome_sprite", "subpixel_sprite", "polychrome_sprite", "emoji_rasterization"] {
        let shader = source.join(if module == "emoji_rasterization" {
            "src/color_text_raster.hlsl"
        } else { "src/shaders.hlsl" });
        for (stage, target, suffix) in [("vertex", "vs_4_1", "VERTEX"), ("fragment", "ps_4_1", "FRAGMENT")] {
            let name = format!("{}_{suffix}_BYTES", module.to_uppercase());
            let header = out.join(format!("{name}.h"));
            let output = Command::new(&fxc).args(["/T", target, "/E", &format!("{module}_{stage}"),
                "/Fh", &windows_path(&header), "/Vn", &name, "/O3", &windows_path(&shader)])
                .output().expect("could not start fxc.exe");
            assert!(output.status.success(), "shader {module}/{stage}: {}{}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            let header = fs::read_to_string(header).unwrap();
            let array = header.split_once("const BYTE").unwrap().1.split_once('=').unwrap().1.trim();
            bindings.push_str(&format!("const {name}: &[u8] = &{}\n", array.replace('{', "[").replace('}', "]")));
        }
    }
    fs::write(out.join("shaders_bytes.rs"), bindings).unwrap();
}
