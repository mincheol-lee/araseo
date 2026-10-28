//! Windows terminal lifetime host. It passes stdio directly to wsl.exe, waits
//! on kernel handles, and owns a private kill-on-close job (never the IDE's job).
#[cfg(windows)]
mod windows_host {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetProcessTimes, OpenEventW, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, SetEvent, WaitForMultipleObjects,
        WaitForSingleObject,
    };

    const FLAG: &str = "--araseo-terminal-host";
    const NO_WINDOW: u32 = 0x08000000;
    static HOST_JOB: OnceLock<OwnedHandle> = OnceLock::new();
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
        }
    }
    fn birth(handle: HANDLE) -> io::Result<u64> {
        let mut created: FILETIME = unsafe { std::mem::zeroed() };
        let (mut exited, mut kernel, mut user) = (created, created, created);
        if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
    pub fn current_birth() -> io::Result<u64> {
        birth(unsafe { GetCurrentProcess() })
    }
    pub fn owner_running(pid: u32, expected: Option<u64>) -> bool {
        let handle = match owned(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        }) {
            Ok(handle) => handle,
            Err(error) => return error.raw_os_error() != Some(ERROR_INVALID_PARAMETER as i32),
        };
        if unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
            return false;
        }
        expected.is_none_or(|expected| {
            birth(handle.as_raw_handle()).map_or(true, |actual| actual == expected)
        })
    }
    pub struct StopEvent {
        handle: OwnedHandle,
        name: String,
    }
    impl StopEvent {
        pub fn new(token: &str) -> io::Result<Self> {
            let name = format!("Local\\AraseoTerminal-{token}");
            let handle =
                owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, wide(&name).as_ptr()) })?;
            Ok(Self { handle, name })
        }
        pub fn signal(&self) {
            unsafe {
                SetEvent(self.handle.as_raw_handle());
            }
        }
        pub fn command(&self, owner_created: u64) -> io::Result<Command> {
            let mut command = Command::new(std::env::current_exe()?);
            command
                .creation_flags(NO_WINDOW)
                .arg(FLAG)
                .arg(std::process::id().to_string())
                .arg(owner_created.to_string())
                .arg(&self.name);
            Ok(command)
        }
    }
    impl Drop for StopEvent {
        fn drop(&mut self) {
            self.signal();
        }
    }

    fn own_job() -> io::Result<()> {
        let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // Keep the job handle until process exit: dropping it here would kill
        // this host too. Only this private host and its children enter the job.
        let _ = HOST_JOB.set(job);
        Ok(())
    }
    fn run() -> anyhow::Result<i32> {
        let args: Vec<_> = std::env::args_os().collect();
        anyhow::ensure!(args.len() >= 7, "invalid terminal host arguments");
        let pid: u32 = args[2]
            .to_str()
            .ok_or_else(|| io::Error::other("invalid owner"))?
            .parse()?;
        let expected: u64 = args[3]
            .to_str()
            .ok_or_else(|| io::Error::other("invalid owner time"))?
            .parse()?;
        let parent = owned(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        })?;
        anyhow::ensure!(
            birth(parent.as_raw_handle())? == expected,
            "terminal owner has exited"
        );
        let event_name = args[4]
            .to_str()
            .ok_or_else(|| io::Error::other("invalid stop event"))?;
        let stop = owned(unsafe { OpenEventW(PROCESS_SYNCHRONIZE, 0, wide(event_name).as_ptr()) })?;
        let handles = [parent.as_raw_handle(), stop.as_raw_handle()];
        if unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 0) } != WAIT_TIMEOUT {
            return Ok(0);
        }
        own_job()?;
        let distro = args[5]
            .to_str()
            .ok_or_else(|| io::Error::other("invalid distro"))?;
        let token = args[6]
            .to_str()
            .ok_or_else(|| io::Error::other("invalid terminal token"))?;
        let mut child = Command::new(r"C:\Windows\System32\wsl.exe")
            .creation_flags(NO_WINDOW)
            .args(["-d", distro])
            .args(&args[7..])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        let handles = [
            parent.as_raw_handle(),
            stop.as_raw_handle(),
            child.as_raw_handle(),
        ];
        let reason = unsafe { WaitForMultipleObjects(3, handles.as_ptr(), 0, u32::MAX) };
        anyhow::ensure!(
            reason >= WAIT_OBJECT_0 && reason < WAIT_OBJECT_0 + 3,
            "terminal lifetime wait failed"
        );
        // This host survives an IDE crash. Cleanup runs here, off the UI thread,
        // before the job closes wsl.exe. Parent handle pins against PID reuse.
        crate::terminal_lifecycle::close(distro, token);
        let code = if reason == WAIT_OBJECT_0 + 2 {
            child.wait()?.code().unwrap_or(1)
        } else {
            0
        };
        Ok(code)
    }
    pub fn run_if_requested() {
        if std::env::args_os().nth(1).is_some_and(|arg| arg == FLAG) {
            let code = match run() {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("Terminal lifetime host: {error:#}");
                    1
                }
            };
            std::process::exit(code);
        }
    }
}
#[cfg(windows)]
pub use windows_host::*;
