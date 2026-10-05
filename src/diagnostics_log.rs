//! Small, local crash breadcrumbs that work without a console or administrator rights.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
static LOG: OnceLock<Mutex<File>> = OnceLock::new();

pub fn path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")));
    base.map(|base| base.join("araseo/logs/araseo.log"))
}

pub fn init() {
    if let Some(path) = path() {
        if let Ok(file) = open_log(&path) {
            let _ = LOG.set(Mutex::new(file));
        }
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let location = panic
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown location".into());
        let message = panic
            .payload()
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| panic.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".into());
        event(&format!(
            "PANIC at {location}: {message}\n{}",
            std::backtrace::Backtrace::force_capture()
        ));
        previous_hook(panic);
    }));
    #[cfg(windows)]
    unsafe {
        // Keep Windows Error Reporting in charge after recording the fault.
        windows_sys::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(
            native_exception,
        ));
    }
    event(&format!(
        "started pid={} version={} build={}",
        std::process::id(),
        env!("CARGO_PKG_VERSION"),
        option_env!("ARASEO_BUILD_REVISION").unwrap_or("unknown")
    ));
}

#[cfg(windows)]
unsafe extern "system" fn native_exception(
    exception: *const windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    // Avoid blocking or allocating while the process may have a damaged heap.
    if let Some(record) = unsafe { exception.as_ref() }
        .and_then(|pointers| unsafe { pointers.ExceptionRecord.as_ref() })
        && let Some(log) = LOG.get()
        && let Ok(mut file) = log.try_lock()
    {
        let address = record.ExceptionAddress as usize;
        let module = MODULES.get().and_then(|modules| {
            modules
                .iter()
                .find(|module| address >= module.base && address - module.base < module.size)
        });
        let base = module.map(|module| module.base).unwrap_or_else(|| {
            let mut info = unsafe {
                std::mem::zeroed::<windows_sys::Win32::System::Memory::MEMORY_BASIC_INFORMATION>()
            };
            if unsafe {
                windows_sys::Win32::System::Memory::VirtualQuery(
                    record.ExceptionAddress,
                    &mut info,
                    std::mem::size_of_val(&info),
                )
            } != 0
            {
                info.AllocationBase as usize
            } else {
                0
            }
        });
        let access = if record.ExceptionCode as u32 == 0xc0000005 && record.NumberParameters >= 2 {
            Some((
                record.ExceptionInformation[0],
                record.ExceptionInformation[1],
            ))
        } else {
            None
        };
        let _ = write_native_details(
            &mut *file,
            record.ExceptionCode as u32,
            address,
            module.map_or("unknown", |module| module.name.as_str()),
            base,
            unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() },
            access,
        );
        let _ = file.flush();
    }
    windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_CONTINUE_SEARCH
}

#[cfg(windows)]
struct Module {
    base: usize,
    size: usize,
    name: String,
}
#[cfg(windows)]
static MODULES: OnceLock<Vec<Module>> = OnceLock::new();

/// Cache module names while the process is healthy; the exception filter only reads it.
pub fn capture_modules() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
        let snapshot =
            CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, std::process::id());
        if snapshot == INVALID_HANDLE_VALUE {
            return;
        }
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut modules = Vec::new();
        let mut available = Module32FirstW(snapshot, &mut entry);
        while available != 0 {
            let end = entry
                .szModule
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(entry.szModule.len());
            modules.push(Module {
                base: entry.modBaseAddr as usize,
                size: entry.modBaseSize as usize,
                name: String::from_utf16_lossy(&entry.szModule[..end]),
            });
            available = Module32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        let _ = MODULES.set(modules);
    }
}

#[cfg(any(windows, test))]
fn write_native_details(
    mut output: impl Write,
    code: u32,
    address: usize,
    module: &str,
    base: usize,
    thread: u32,
    access: Option<(usize, usize)>,
) -> io::Result<()> {
    write!(
        output,
        "NATIVE_EXCEPTION code=0x{code:08x} address=0x{address:016x} thread={thread} module={module} base=0x{base:016x} rva=0x{:x}",
        address.saturating_sub(base)
    )?;
    if let Some((kind, target)) = access {
        let kind = match kind {
            0 => "read",
            1 => "write",
            8 => "execute",
            _ => "unknown",
        };
        write!(output, " access={kind} target=0x{target:016x}")?;
    }
    writeln!(output)
}

pub fn event(message: &str) {
    if let Some(log) = LOG.get()
        && let Ok(mut file) = log.lock()
    {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let _ = writeln!(file, "{timestamp} {message}");
    }
}

fn open_log(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES) {
        let previous = path.with_file_name("araseo.previous.log");
        // Windows cannot rename over an existing file.
        let _ = fs::remove_file(&previous);
        // Another running instance may have the file open on Windows. Keep
        // logging to the current file if rotation is temporarily unavailable.
        let _ = fs::rename(path, previous);
    }
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_a_full_log_and_keeps_a_new_writable_file() {
        let directory =
            std::env::temp_dir().join(format!("araseo-log-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("araseo.log");
        fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();

        let mut file = open_log(&path).unwrap();
        writeln!(file, "new session").unwrap();
        assert_eq!(
            fs::metadata(directory.join("araseo.previous.log"))
                .unwrap()
                .len(),
            MAX_LOG_BYTES
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "new session\n");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn records_native_exception_code_and_address() {
        let mut output = Vec::new();
        write_native_details(
            &mut output,
            0xc000_0005,
            0x1234,
            "araseo.exe",
            0x1000,
            7,
            Some((1, 0xdead)),
        )
        .unwrap();
        assert_eq!(
            output,
            b"NATIVE_EXCEPTION code=0xc0000005 address=0x0000000000001234 thread=7 module=araseo.exe base=0x0000000000001000 rva=0x234 access=write target=0x000000000000dead\n"
        );
    }
}
