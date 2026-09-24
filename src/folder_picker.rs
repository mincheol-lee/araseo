//! Native Windows folder picker and translation into the active WSL distro.

use crate::workspace::{Workspace, wsl_unc_to_linux_path};
use anyhow::{Context, Result, bail};
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, PCWSTR, w};

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// Called on a dedicated thread so the Explorer dialog has its own STA and
/// the Slint event loop remains free to repaint.
pub fn pick_folder(owner: usize, workspace: &Workspace) -> Result<Option<PathBuf>> {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .context("cannot initialize the Windows folder picker")?;
    let _apartment = ComApartment;
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
            .context("cannot create the Windows folder picker")?;
    unsafe {
        dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS)?;
        dialog.SetTitle(w!("Select a folder to add to Araseo"))?;
    }

    let initial = workspace
        .host_root
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    if let Ok(folder) =
        unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(initial.as_ptr()), None) }
    {
        let _ = unsafe { dialog.SetDefaultFolder(&folder) };
    }

    let owner = HWND(owner as *mut c_void);
    if let Err(error) = unsafe { dialog.Show(Some(owner)) } {
        if error.code() == HRESULT(0x8007_04c7u32 as i32) {
            return Ok(None);
        }
        return Err(error).context("Windows folder picker failed");
    }
    let item = unsafe { dialog.GetResult() }.context("cannot read selected folder")?;
    let wide = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
        .context("selected item is not a filesystem folder")?;
    let selected = unsafe { wide.to_string() };
    unsafe { CoTaskMemFree(Some(wide.0 as *const c_void)) };
    let selected = selected.context("selected folder path is not valid Unicode")?;
    selected_folder_to_linux_path(&workspace.distro, &selected).map(Some)
}

fn selected_folder_to_linux_path(distro: &str, selected: &str) -> Result<PathBuf> {
    if let Some(path) = wsl_unc_to_linux_path(distro, selected)? {
        return Ok(path);
    }
    if !PathBuf::from(selected).is_absolute() {
        bail!("select a folder in WSL or on a Windows drive");
    }
    let output = Command::new("wsl.exe")
        .args(["-d", distro, "--exec", "wslpath", "-u"])
        .arg(selected)
        .output()
        .context("cannot convert the selected Windows folder to a WSL path")?;
    if !output.status.success() {
        bail!("the selected folder is not accessible from the current WSL distribution");
    }
    let linux = String::from_utf8(output.stdout).context("WSL returned an invalid path")?;
    let linux = linux.trim();
    if !linux.starts_with('/') {
        bail!("WSL could not resolve the selected folder");
    }
    Ok(PathBuf::from(linux))
}
