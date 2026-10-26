use crate::terminal_input::TerminalInput;
#[path = "terminal_output.rs"]
mod output;
use anyhow::{Context, Result};
use output::TerminalOutput;
pub use output::{FrameRow, TerminalFrame};
#[cfg(not(target_os = "windows"))]
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
#[cfg(windows)]
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
static NEXT_TERMINAL_ID: AtomicU32 = AtomicU32::new(1);

#[cfg(any(windows, test))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// One process-only record per TTY path, including inaccessible terminals.
#[cfg(any(windows, test))]
const AGENT_PROCESS_PROBE: &str = "for tty_path_file do tty=$(cat \"$tty_path_file\" 2>/dev/null); ps -t \"$tty\" -o comm=,args= 2>/dev/null; printf '\\036'; done";

// Keep CLI-specific keyboard/IME compatibility without adding configuration or
// hook arguments, writing activity files, or changing saved user settings.
fn terminal_shell_command() -> &'static str {
    r#"codex() { CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT="${CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT:-1}" command codex "$@"; }; claude() { CLAUDE_CODE_NATIVE_CURSOR="${CLAUDE_CODE_NATIVE_CURSOR:-1}" command claude "$@"; }; export -f codex claude; exec /bin/bash --login -i"#
}

// Deliver the first change immediately; cap subsequent requests to roughly
// 15 Hz on the 33 ms terminal timer without waiting for dragging to stop.
#[cfg(any(target_os = "windows", test))]
struct ResizeThrottle {
    polls_since_dispatch: u8,
}

#[cfg(any(target_os = "windows", test))]
impl Default for ResizeThrottle {
    fn default() -> Self {
        Self {
            polls_since_dispatch: 2,
        }
    }
}

#[cfg(any(target_os = "windows", test))]
impl ResizeThrottle {
    fn observe(&mut self) -> bool {
        self.polls_since_dispatch = self.polls_since_dispatch.saturating_add(1);
        self.polls_since_dispatch >= 2
    }
    fn retry_later(&mut self) {
        self.polls_since_dispatch = 0;
    }
}

#[cfg(any(target_os = "windows", test))]
struct WindowsPtyResize {
    request_sender: mpsc::SyncSender<(u16, u16)>,
    result_receiver: mpsc::Receiver<((u16, u16), bool)>,
    in_flight: Option<(u16, u16)>,
    applied: (u16, u16),
    throttle: ResizeThrottle,
}

#[cfg(any(target_os = "windows", test))]
impl WindowsPtyResize {
    #[cfg(target_os = "windows")]
    fn new(distro: String, tty_path_file: String) -> std::io::Result<Self> {
        Self::start_with(move || {
            // Start the private pipe off the UI thread when the terminal opens.
            let mut transport = crate::terminal_resize::WslResize::new(distro, tty_path_file);
            transport.prepare();
            move |rows, columns| transport.resize(rows, columns)
        })
    }

    #[cfg(test)]
    fn start(resize: impl FnMut(u16, u16) -> bool + Send + 'static) -> std::io::Result<Self> {
        Self::start_with(move || resize)
    }

    fn start_with<F: FnMut(u16, u16) -> bool + Send + 'static>(
        initialize: impl FnOnce() -> F + Send + 'static,
    ) -> std::io::Result<Self> {
        let (request_sender, request_receiver) = mpsc::sync_channel(1);
        let (result_sender, result_receiver) = mpsc::channel();
        thread::Builder::new()
            .name("araseo-resize".into())
            .spawn(move || {
                let mut resize = initialize();
                while let Ok(size @ (rows, columns)) = request_receiver.recv() {
                    let resized = resize(rows, columns);
                    if result_sender.send((size, resized)).is_err() {
                        break;
                    }
                }
            })?;

        Ok(Self {
            request_sender,
            result_receiver,
            in_flight: None,
            applied: (24, 80),
            throttle: ResizeThrottle::default(),
        })
    }

    fn apply(&mut self, rows: u16, columns: u16) {
        // Always drain completions, including when the local grid has already
        // returned to its original size during an in-flight WSL request.
        if let Ok((completed, resized)) = self.result_receiver.try_recv() {
            self.in_flight = None;
            if resized {
                self.applied = completed;
            } else {
                self.throttle.retry_later();
            }
        }
        let ready = self.throttle.observe();
        if self.in_flight.is_some() {
            return;
        }
        let requested = (rows, columns);
        if self.applied == requested {
            self.throttle = ResizeThrottle::default();
            return;
        }
        if !ready {
            return;
        }
        if self.request_sender.try_send(requested).is_ok() {
            self.in_flight = Some(requested);
        }
        self.throttle.retry_later();
    }
}

const DEFAULT_FOREGROUND: [u8; 3] = [0xff, 0xff, 0xff];
const DEFAULT_BACKGROUND: [u8; 3] = [0x28, 0x2c, 0x34];

#[derive(Default)]
struct TerminalCallbacks {
    replies: Vec<u8>,
}

impl vt100::Callbacks for TerminalCallbacks {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        let [parameter] = params else {
            return;
        };
        let [value] = *parameter else {
            return;
        };
        match (i1, i2, command, *value) {
            (None, None, 'n', 5) => self.replies.extend_from_slice(b"\x1b[0n"),
            (None, None, 'n', 6) | (Some(b'?'), None, 'n', 6) => {
                // Codex queries CPR during resize before accepting more input.
                let (row, column) = screen.cursor_position_for_report();
                let private = if i1 == Some(b'?') { "?" } else { "" };
                self.replies.extend_from_slice(
                    format!(
                        "\x1b[{private}{};{}R",
                        u32::from(row) + 1,
                        u32::from(column) + 1
                    )
                    .as_bytes(),
                );
            }
            (None, None, 't', 18) => {
                let (rows, columns) = screen.size();
                self.replies
                    .extend_from_slice(format!("\x1b[8;{rows};{columns}t").as_bytes());
            }
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _screen: &mut vt100::Screen, params: &[&[u8]]) {
        let (slot, color) = match params {
            [b"10", b"?"] => (10, DEFAULT_FOREGROUND),
            [b"11", b"?"] => (11, DEFAULT_BACKGROUND),
            _ => return,
        };
        let [red, green, blue] = color.map(|channel| u16::from(channel) * 0x101);
        self.replies.extend_from_slice(
            format!("\x1b]{slot};rgb:{red:04x}/{green:04x}/{blue:04x}\x1b\\").as_bytes(),
        );
    }
}

fn write_terminal_replies(writer: &mut impl Write, callbacks: &mut TerminalCallbacks) {
    if callbacks.replies.is_empty() {
        return;
    }
    let replies = std::mem::take(&mut callbacks.replies);
    let _ = writer.write_all(&replies);
    let _ = writer.flush();
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayCell {
    pub row: i32,
    pub column: i32,
    pub glyph: String,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub bold: bool,
    pub cursor: bool,
    pub column_span: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TerminalWheel {
    /// Positive rows scroll toward older output (wheel up); negative toward newer.
    pub rows: i32,
    pub row: i32,
    pub column: i32,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

type OutputWaker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct OutputSignal {
    pending: AtomicBool,
    waker: Mutex<Option<OutputWaker>>,
}

impl OutputSignal {
    fn notify(&self) {
        if self.pending.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        let waker = self.waker.lock().ok().and_then(|waker| waker.clone());
        if let Some(waker) = waker {
            waker();
        }
    }

    fn set_waker(&self, waker: OutputWaker) {
        if let Ok(mut current) = self.waker.lock() {
            *current = Some(waker.clone());
        }
        if self.pending.load(AtomicOrdering::Acquire) {
            waker();
        }
    }

    fn clear_pending(&self) {
        self.pending.store(false, AtomicOrdering::Release);
    }
}

// Thread creation can fail after the shell starts. Kill that child on every
// early return, and transfer ownership only once construction has succeeded.
#[derive(Default)]
struct SpawnCleanup {
    pipe: Option<std::process::Child>,
    pty: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}
impl Drop for SpawnCleanup {
    fn drop(&mut self) {
        if let Some(child) = &mut self.pipe {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(child) = &mut self.pty {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct TerminalSession {
    agent_probe: AgentProbe,
    #[cfg(not(target_os = "windows"))]
    master: Box<dyn MasterPty + Send>,
    writer: TerminalInput,
    output: TerminalOutput,
    requested_size: (u16, u16),
    _pty_child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    _pipe_child: Option<std::process::Child>,
    #[cfg(target_os = "windows")]
    windows_resize: WindowsPtyResize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentKind {
    Codex,
    Claude,
}

#[derive(Clone)]
pub struct AgentProbe {
    tty: String,
    #[cfg(windows)]
    distro: String,
}

impl AgentProbe {
    pub fn detect_many(
        probes: &[(crate::tabs::TabId, Self)],
    ) -> Vec<(crate::tabs::TabId, Option<AgentKind>)> {
        #[cfg(not(windows))]
        {
            probes
                .iter()
                .map(|(id, probe)| (*id, probe.detect()))
                .collect()
        }
        #[cfg(windows)]
        {
            if probes.is_empty() {
                return Vec::new();
            }
            let mut command = Command::new(r"C:\Windows\System32\wsl.exe");
            command.creation_flags(CREATE_NO_WINDOW);
            command.args([
                "-d",
                &probes[0].1.distro,
                "--exec",
                "/bin/sh",
                "-c",
                AGENT_PROCESS_PROBE,
                "araseo-agent-batch",
            ]);
            for (_, probe) in probes {
                command.arg(&probe.tty);
            }
            let output =
                crate::process_job::output(&mut command, std::time::Duration::from_secs(3));
            let records = output
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                .unwrap_or_default();
            let mut records = records.split('\x1e');
            probes
                .iter()
                .map(|(id, _)| {
                    let status = records.next().and_then(detect_agent);
                    (*id, status)
                })
                .collect()
        }
    }

    #[cfg(not(windows))]
    pub fn detect(&self) -> Option<AgentKind> {
        let output = crate::process_job::output(
            Command::new("ps").args(["-t", &self.tty, "-o", "comm=,args="]),
            std::time::Duration::from_secs(3),
        )
        .ok()?;
        output
            .status
            .success()
            .then(|| detect_agent(&String::from_utf8_lossy(&output.stdout)))?
    }
}

fn detect_agent(processes: &str) -> Option<AgentKind> {
    let mut found = None;
    for line in processes.lines() {
        let mut fields = line.split_whitespace();
        let Some(command) = fields.next() else {
            continue;
        };
        let executable = command.rsplit('/').next().unwrap_or(command);
        if executable == "codex" || executable.starts_with("codex-") {
            found = Some(AgentKind::Codex);
        } else if executable == "claude" || executable.starts_with("claude-") {
            found = Some(AgentKind::Claude);
        } else if executable == "node" {
            let args = fields.collect::<Vec<_>>().join(" ");
            if args.contains("/@openai/codex/") {
                found = Some(AgentKind::Codex);
            } else if args.contains("/@anthropic-ai/claude-code/") {
                found = Some(AgentKind::Claude);
            }
        }
    }
    found
}

impl TerminalSession {
    /// UI tests inspect bytes after native input dispatch without depending on
    /// a shell's echo, buffering, or keyboard protocol negotiation.
    #[cfg(feature = "ui-tests")]
    pub fn capture_input(&mut self) -> std::io::Result<mpsc::Receiver<Vec<u8>>> {
        struct Recorder(mpsc::Sender<Vec<u8>>);
        impl Write for Recorder {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .send(bytes.to_vec())
                    .map_err(|_| std::io::ErrorKind::BrokenPipe)?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (sender, receiver) = mpsc::channel();
        self.writer = TerminalInput::new(Recorder(sender))?;
        Ok(receiver)
    }
    pub fn spawn(_distro: &str, linux_root: &Path) -> Result<Self> {
        #[cfg(target_os = "windows")]
        {
            return Self::spawn_windows(_distro, linux_root);
        }

        #[cfg(not(target_os = "windows"))]
        Self::spawn_pty(linux_root)
    }

    #[cfg(target_os = "windows")]
    fn spawn_windows(distro: &str, linux_root: &Path) -> Result<Self> {
        let terminal_id = NEXT_TERMINAL_ID.fetch_add(1, Ordering::Relaxed);
        let tty_path_file = format!("/tmp/araseo-pty-{}-{terminal_id}", std::process::id());
        let shell = terminal_shell_command();
        let shell_command = format!(
            "stty rows 24 cols 80; tty > {tty_path_file}; \
             /usr/bin/env TERM=xterm-256color COLORTERM=truecolor /bin/bash -c {}; \
             araseo_status=$?; rm -f {tty_path_file}; exit $araseo_status",
            shell_quote(shell)
        );
        let mut command = std::process::Command::new(r"C:\Windows\System32\wsl.exe");
        command.creation_flags(CREATE_NO_WINDOW);
        command
            .args(["-d", distro, "--cd"])
            .arg(linux_root)
            .args(["--exec", "/usr/bin/script", "-qfec"])
            .arg(shell_command)
            .arg("/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut cleanup = SpawnCleanup {
            pipe: Some(command.spawn().context("failed to start WSL shell")?),
            pty: None,
        };
        let child = cleanup.pipe.as_mut().unwrap();
        let stdout = child.stdout.take().context("WSL stdout is unavailable")?;
        let stderr = child.stderr.take().context("WSL stderr is unavailable")?;
        let stdin = child.stdin.take().context("WSL stdin is unavailable")?;

        let writer = TerminalInput::new(stdin)?;
        let output = TerminalOutput::spawn(24, 80, writer.clone())?;
        spawn_reader(stdout, output.sender.clone())?;
        spawn_reader(stderr, output.sender.clone())?;

        let windows_resize = WindowsPtyResize::new(distro.to_string(), tty_path_file.clone())?;
        Ok(Self {
            agent_probe: AgentProbe {
                tty: tty_path_file.clone(),
                distro: distro.to_string(),
            },
            writer,
            output,
            // Match util-linux script's initial PTY grid.
            requested_size: (24, 80),
            _pty_child: None,
            _pipe_child: cleanup.pipe.take(),
            windows_resize,
        })
    }

    #[cfg(not(target_os = "windows"))]
    fn spawn_pty(linux_root: &Path) -> Result<Self> {
        let pair = native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut command = CommandBuilder::new("bash");
        command.arg("-c");
        command.arg(terminal_shell_command());
        command.cwd(linux_root);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let mut cleanup = SpawnCleanup {
            pty: Some(
                pair.slave
                    .spawn_command(command)
                    .context("failed to start WSL shell")?,
            ),
            pipe: None,
        };
        let reader = pair.master.try_clone_reader()?;
        let writer = TerminalInput::new(pair.master.take_writer()?)?;
        let output = TerminalOutput::spawn(24, 100, writer.clone())?;
        spawn_reader(reader, output.sender.clone())?;

        Ok(Self {
            agent_probe: AgentProbe {
                tty: pair
                    .master
                    .tty_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            },
            master: pair.master,
            writer,
            output,
            requested_size: (24, 100),
            _pty_child: cleanup.pty.take(),
            _pipe_child: None,
        })
    }

    pub fn poll(&mut self) -> bool {
        self.output.poll()
    }

    pub fn take_input_error(&mut self) -> Option<String> {
        self.writer.take_error()
    }

    pub fn agent_probe(&self) -> AgentProbe {
        self.agent_probe.clone()
    }

    pub fn distribution(&self) -> &str {
        #[cfg(windows)]
        {
            &self.agent_probe.distro
        }
        #[cfg(not(windows))]
        {
            ""
        }
    }

    pub fn set_output_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.output.set_waker(waker);
    }

    pub fn set_visible(&self, visible: bool) {
        self.output.set_visible(visible);
    }

    pub fn frame(&self) -> Arc<TerminalFrame> {
        self.output.frame.clone()
    }

    pub fn cells(&self) -> Vec<DisplayCell> {
        self.output
            .frame
            .rows
            .iter()
            .flat_map(|row| row.cells.iter().cloned())
            .collect()
    }

    pub fn size(&self) -> (u16, u16) {
        self.requested_size
    }

    pub fn cursor_row(&self) -> i32 {
        self.output.frame.cursor.map_or(-1, |(row, _)| row.into())
    }

    pub fn cursor_column(&self) -> i32 {
        self.output
            .frame
            .cursor
            .map_or(-1, |(_, column)| column.into())
    }

    pub fn selection_text(
        &self,
        anchor_row: i32,
        anchor_column: i32,
        cursor_row: i32,
        cursor_column: i32,
    ) -> String {
        self.output
            .frame
            .selection_text(anchor_row, anchor_column, cursor_row, cursor_column)
    }

    pub fn scroll_scrollback(&mut self, wheel: TerminalWheel) -> bool {
        self.output.scroll(wheel)
    }

    pub fn write(&mut self, bytes: &[u8]) -> bool {
        let returned_to_bottom = self.output.frame.scrollback > 0;
        self.output.bottom();
        let _ = self.writer.write_all(bytes);
        returned_to_bottom
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> bool {
        let (rows, cols) = (rows.max(1), cols.max(1));
        #[cfg(target_os = "windows")]
        self.windows_resize.apply(rows, cols);
        if self.requested_size == (rows, cols) {
            return false;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        // The parser receives a coalesced size command independently of
        // presentation; the PTY notifies the foreground TUI to redraw.
        self.requested_size = (rows, cols);
        self.output.resize(rows, cols);
        true
    }
}

// Cursor visibility controls painting, not where IME composition belongs.
fn input_cursor_position(screen: &vt100::Screen) -> Option<(u16, u16)> {
    (screen.scrollback() == 0).then(|| screen.cursor_position())
}

#[cfg(test)]
fn display_cells(screen: &vt100::Screen) -> Vec<DisplayCell> {
    (0..screen.size().0)
        .flat_map(|row| display_row(screen, row))
        .collect()
}

fn display_row(screen: &vt100::Screen, row: u16) -> Vec<DisplayCell> {
    let (_, columns) = screen.size();
    let cursor_position =
        (!screen.hide_cursor() && screen.scrollback() == 0).then(|| screen.cursor_position());
    let mut cells = Vec::new();
    for column in 0..columns {
        let cell = screen.cell(row, column);
        let color = cell.map(vt100::Cell::fgcolor).unwrap_or_default();
        // Match Orca/xterm's bright ANSI colors for bold shell output.
        let color = match color {
            vt100::Color::Idx(index @ 0..=7) if cell.is_some_and(vt100::Cell::bold) => {
                vt100::Color::Idx(index + 8)
            }
            color => color,
        };
        let mut foreground = terminal_color(color, DEFAULT_FOREGROUND);
        let mut background = terminal_color(
            cell.map(vt100::Cell::bgcolor).unwrap_or_default(),
            DEFAULT_BACKGROUND,
        );
        if cell.is_some_and(vt100::Cell::inverse) {
            std::mem::swap(&mut foreground, &mut background);
        }
        if cell.is_some_and(vt100::Cell::dim) {
            foreground = foreground.map(|channel| channel.saturating_mul(2) / 3);
        }
        let glyph = cell
            .map(vt100::Cell::contents)
            .filter(|contents| !contents.is_empty())
            .unwrap_or(" ");
        let cursor = cursor_position == Some((row, column));
        let column_span = cell_column_span(cell);
        if column_span == 0 || (!cursor && glyph == " " && background == DEFAULT_BACKGROUND) {
            continue;
        }
        cells.push(DisplayCell {
            row: row.into(),
            column: column.into(),
            glyph: glyph.to_string(),
            foreground,
            background,
            bold: cell.is_some_and(vt100::Cell::bold),
            cursor,
            column_span,
        });
    }
    cells
}

fn scroll_screen(screen: &mut vt100::Screen, rows: i32) -> bool {
    let current = screen.scrollback();
    let requested = if rows >= 0 {
        current.saturating_add(rows as usize)
    } else {
        current.saturating_sub(rows.unsigned_abs() as usize)
    };
    screen.set_scrollback(requested);
    screen.scrollback() != current
}

#[cfg(test)]
fn selection_text(
    screen: &vt100::Screen,
    anchor_row: i32,
    anchor_column: i32,
    cursor_row: i32,
    cursor_column: i32,
) -> String {
    let (rows, columns) = screen.size();
    if rows == 0 || columns == 0 {
        return String::new();
    }

    let clamp_row = |row: i32| row.clamp(0, i32::from(rows) - 1) as u16;
    let clamp_column = |column: i32| column.clamp(0, i32::from(columns) - 1) as u16;
    let anchor = (clamp_row(anchor_row), clamp_column(anchor_column));
    let cursor = (clamp_row(cursor_row), clamp_column(cursor_column));
    let (start, end) = if anchor <= cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    };

    // The UI identifies the cells under the two pointer positions. vt100's
    // end coordinate is exclusive, so include the final selected cell.
    screen.contents_between(start.0, start.1, end.0, end.1.saturating_add(1))
}

fn mouse_scroll_input(screen: &vt100::Screen, wheel: TerminalWheel) -> Option<Vec<u8>> {
    if wheel.shift
        || wheel.rows == 0
        || matches!(
            screen.mouse_protocol_mode(),
            vt100::MouseProtocolMode::None | vt100::MouseProtocolMode::Press
        )
    {
        return None;
    }

    // Follow the modes requested by the program, in either screen buffer.
    // Mouse reports are one-based; legacy protocols cannot encode large cells.
    let (rows, columns) = screen.size();
    let row = wheel.row.clamp(0, i32::from(rows.saturating_sub(1))) as u32 + 1;
    let column = wheel.column.clamp(0, i32::from(columns.saturating_sub(1))) as u32 + 1;
    let button = if wheel.rows > 0 { 64 } else { 65 }
        | if wheel.alt { 8 } else { 0 }
        | if wheel.control { 16 } else { 0 };
    let report = match screen.mouse_protocol_encoding() {
        vt100::MouseProtocolEncoding::Sgr => format!("\x1b[<{button};{column};{row}M").into_bytes(),
        vt100::MouseProtocolEncoding::Default => {
            if row > 223 || column > 223 {
                return None;
            }
            vec![
                0x1b,
                b'[',
                b'M',
                button + 32,
                (column + 32) as u8,
                (row + 32) as u8,
            ]
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            if row > 2015 || column > 2015 {
                return None;
            }
            let mut report = String::from("\x1b[M");
            for value in [u32::from(button) + 32, column + 32, row + 32] {
                report.push(char::from_u32(value)?);
            }
            report.into_bytes()
        }
    };
    Some(report.repeat(wheel.rows.unsigned_abs().min(100) as usize))
}

fn report_mouse_scroll(screen: &mut vt100::Screen, writer: &mut impl Write, input: &[u8]) -> bool {
    if writer
        .write_all(input)
        .and_then(|()| writer.flush())
        .is_err()
    {
        return false;
    }
    // A mouse report refers to the live grid, not a local history row.
    let returned_to_bottom = screen.scrollback() > 0;
    screen.set_scrollback(0);
    returned_to_bottom
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Some(child) = self._pty_child.as_mut() {
            let _ = child.kill();
        }
        if let Some(child) = self._pipe_child.as_mut() {
            let _ = child.kill();
        }
    }
}

fn cell_column_span(cell: Option<&vt100::Cell>) -> i32 {
    if cell.is_some_and(vt100::Cell::is_wide_continuation) {
        0
    } else if cell.is_some_and(vt100::Cell::is_wide) {
        2
    } else {
        1
    }
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    sender: mpsc::SyncSender<output::Event>,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("araseo-terminal-output".into())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if sender
                            .send(output::Event::Output(buffer[..count].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        })?;
    Ok(())
}

fn terminal_color(color: vt100::Color, default: [u8; 3]) -> [u8; 3] {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(red, green, blue) => [red, green, blue],
        vt100::Color::Idx(index @ 0..=15) => ANSI_COLORS[index as usize],
        vt100::Color::Idx(index @ 16..=231) => {
            let value = index - 16;
            let levels = [0, 95, 135, 175, 215, 255];
            [
                levels[(value / 36) as usize],
                levels[((value % 36) / 6) as usize],
                levels[(value % 6) as usize],
            ]
        }
        vt100::Color::Idx(index) => {
            let gray = 8 + (index - 232) * 10;
            [gray, gray, gray]
        }
    }
}

// Orca: Ghostty Default Style Dark.
const ANSI_COLORS: [[u8; 3]; 16] = [
    [0x1d, 0x1f, 0x21],
    [0xcc, 0x66, 0x66],
    [0xb5, 0xbd, 0x68],
    [0xf0, 0xc6, 0x74],
    [0x81, 0xa2, 0xbe],
    [0xb2, 0x94, 0xbb],
    [0x8a, 0xbe, 0xb7],
    [0xc5, 0xc8, 0xc6],
    [0x66, 0x66, 0x66],
    [0xd5, 0x4e, 0x53],
    [0xb9, 0xca, 0x4a],
    [0xe7, 0xc5, 0x47],
    [0x7a, 0xa6, 0xda],
    [0xc3, 0x97, 0xd8],
    [0x70, 0xc0, 0xb1],
    [0xea, 0xea, 0xea],
];

/// Xterm functional-key modifiers and CSI-u for otherwise ambiguous keys.
/// Ordinary text and C0 shortcuts remain compatible with shells. We do not
/// advertise Kitty event/release reporting, which this terminal cannot emit.
pub fn encode_key(text: &str, control: bool, alt: bool, shift: bool) -> Vec<u8> {
    if matches!(
        text,
        "\u{10}"
            | "\u{11}"
            | "\u{12}"
            | "\u{13}"
            | "\u{14}"
            | "\u{15}"
            | "\u{16}"
            | "\u{17}"
            | "\u{18}"
            | "shift"
            | "control"
            | "alt"
            | "super"
            | "fn"
            | "capslock"
    ) {
        return Vec::new();
    }
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(control);
    let cursor = match text {
        "<UP>" => Some('A'),
        "<DOWN>" => Some('B'),
        "<RIGHT>" => Some('C'),
        "<LEFT>" => Some('D'),
        "<HOME>" => Some('H'),
        "<END>" => Some('F'),
        "<F1>" => Some('P'),
        "<F2>" => Some('Q'),
        "<F3>" => Some('R'),
        "<F4>" => Some('S'),
        _ => None,
    };
    if let Some(final_byte) = cursor {
        return if modifier != 1 {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else if matches!(text, "<F1>" | "<F2>" | "<F3>" | "<F4>") {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        };
    }
    let numbered = match text {
        "<INSERT>" => Some(2),
        "<DELETE>" => Some(3),
        "<PAGEUP>" => Some(5),
        "<PAGEDOWN>" => Some(6),
        "<F5>" => Some(15),
        "<F6>" => Some(17),
        "<F7>" => Some(18),
        "<F8>" => Some(19),
        "<F9>" => Some(20),
        "<F10>" => Some(21),
        "<F11>" => Some(23),
        "<F12>" => Some(24),
        _ => None,
    };
    if let Some(number) = numbered {
        return if modifier == 1 {
            format!("\x1b[{number}~").into_bytes()
        } else {
            format!("\x1b[{number};{modifier}~").into_bytes()
        };
    }
    let special = match text {
        "<ENTER>" | "\r" | "\n" => Some(13),
        "<TAB>" | "<BACKTAB>" | "\u{19}" => Some(9),
        "<BACKSPACE>" => Some(127),
        "<ESCAPE>" => Some(27),
        _ => None,
    };
    if alt && !control && text.len() == 1 {
        let byte = if shift {
            text.as_bytes()[0].to_ascii_uppercase()
        } else {
            text.as_bytes()[0]
        };
        // ESC [, ESC ], and SS3/string introducers can leave a TUI parser
        // waiting for a sequence terminator instead of reading the next key.
        if matches!(byte, b'[' | b']' | b'O' | b'P' | b'X' | b'^' | b'_') {
            let code = if shift && byte.is_ascii_alphabetic() {
                byte.to_ascii_lowercase()
            } else {
                byte
            };
            return format!("\x1b[{};{modifier}u", u32::from(code)).into_bytes();
        }
    }
    let mut bytes = if let Some(code) = special {
        if code == 9 && (shift || matches!(text, "<BACKTAB>" | "\u{19}")) && !control && !alt {
            return b"\x1b[Z".to_vec();
        }
        if control || shift {
            return format!("\x1b[{code};{modifier}u").into_bytes();
        }
        vec![code as u8]
    } else if control && text.len() == 1 {
        let byte = text.as_bytes()[0].to_ascii_lowercase();
        if shift {
            return format!("\x1b[{};{modifier}u", u32::from(byte)).into_bytes();
        }
        match byte {
            b'a'..=b'z' => vec![byte - b'a' + 1],
            b' ' | b'@' | b'2' => vec![0],
            b'[' | b'3' => vec![27],
            b'\\' | b'4' => vec![28],
            b']' | b'5' => vec![29],
            b'^' | b'6' => vec![30],
            b'_' | b'/' | b'7' => vec![31],
            b'?' | b'8' => vec![127],
            _ => return format!("\x1b[{};{modifier}u", u32::from(byte)).into_bytes(),
        }
    } else if alt && shift && text.len() == 1 {
        vec![text.as_bytes()[0].to_ascii_uppercase()]
    } else {
        text.as_bytes().to_vec()
    };
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_agent_processes_on_the_terminal() {
        assert_eq!(
            detect_agent("bash bash\nps ps -t /dev/pts/2 -o comm=,args=\n"),
            None
        );
        assert_eq!(
            detect_agent("codex /home/user/.cargo/bin/codex\n"),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            detect_agent("claude /home/user/.local/bin/claude\n"),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            detect_agent(
                "node node /home/user/.npm/lib/node_modules/@anthropic-ai/claude-code/cli.js\n"
            ),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            detect_agent("node node /home/user/.npm/lib/node_modules/@openai/codex/bin/codex.js\n"),
            Some(AgentKind::Codex)
        );
        assert_eq!(detect_agent("bash bash -c echo codex\n"), None);
    }

    #[cfg(unix)]
    #[test]
    fn process_probe_keeps_terminal_records_aligned_without_activity_files() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("araseo-process-probe-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let ps = directory.join("ps");
        std::fs::write(
            &ps,
            r#"#!/bin/sh
case "$2" in
 codex-tty) printf 'codex codex\n';;
 claude-tty) printf 'claude claude\n';;
 *) exit 1;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(ps, std::fs::Permissions::from_mode(0o700)).unwrap();
        let codex = directory.join("codex tty");
        let claude = directory.join("claude tty");
        std::fs::write(&codex, "codex-tty").unwrap();
        std::fs::write(&claude, "claude-tty").unwrap();
        let output = Command::new("sh")
            .args(["-c", AGENT_PROCESS_PROBE, "araseo-agent-batch"])
            .arg(codex)
            .arg(directory.join("missing tty"))
            .arg(claude)
            .env(
                "PATH",
                format!("{}:{}", directory.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            text.split('\x1e')
                .take(3)
                .map(detect_agent)
                .collect::<Vec<_>>(),
            vec![Some(AgentKind::Codex), None, Some(AgentKind::Claude)]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cli_compatibility_preserves_arguments_and_exit_codes_without_injected_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("araseo-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["codex", "claude"] {
            let path = directory.join(name);
            std::fs::write(&path, format!(r#"#!/bin/sh
printf '%s\n' "$@" > "$ARASEO_TEST_OUTPUT/{name}-args"
printf '%s\n' "${{CLAUDE_CODE_NATIVE_CURSOR-unset}}" > "$ARASEO_TEST_OUTPUT/{name}-cursor"
printf '%s\n' "${{CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT-unset}}" > "$ARASEO_TEST_OUTPUT/{name}-keyboard"
printf '%s\n' "${{ARASEO_AGENT_STATE_FILE-unset}}" "${{ARASEO_CODEX_HOOKS-unset}}" "${{ARASEO_CLAUDE_SETTINGS-unset}}" > "$ARASEO_TEST_OUTPUT/{name}-injected"
exit 17
"#)).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let script = terminal_shell_command().replace("exec /bin/bash --login -i", r#"codex --help 'two words' ''; printf 'codex=%s\n' "$?"; claude --version '한글 인수'; printf 'claude=%s\n' "$?""#);
        let execute = |overrides: bool| {
            let mut command = Command::new("bash");
            command
                .arg("-c")
                .arg(&script)
                .env_remove("CLAUDE_CODE_NATIVE_CURSOR")
                .env_remove("CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT")
                .env_remove("ARASEO_AGENT_STATE_FILE")
                .env_remove("ARASEO_CODEX_HOOKS")
                .env_remove("ARASEO_CLAUDE_SETTINGS")
                .env("ARASEO_TEST_OUTPUT", &directory)
                .env(
                    "PATH",
                    format!("{}:{}", directory.display(), std::env::var("PATH").unwrap()),
                );
            if overrides {
                command
                    .env("CLAUDE_CODE_NATIVE_CURSOR", "0")
                    .env("CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT", "0");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "codex=17\nclaude=17\n"
            );
            assert_eq!(
                std::fs::read_to_string(directory.join("codex-args")).unwrap(),
                "--help\ntwo words\n\n"
            );
            assert_eq!(
                std::fs::read_to_string(directory.join("claude-args")).unwrap(),
                "--version\n한글 인수\n"
            );
            for name in ["codex", "claude"] {
                assert_eq!(
                    std::fs::read_to_string(directory.join(format!("{name}-injected"))).unwrap(),
                    "unset\nunset\nunset\n"
                );
            }
        };
        execute(false);
        assert_eq!(
            std::fs::read_to_string(directory.join("codex-keyboard"))
                .unwrap()
                .trim(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("claude-cursor"))
                .unwrap()
                .trim(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("codex-cursor"))
                .unwrap()
                .trim(),
            "unset"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("claude-keyboard"))
                .unwrap()
                .trim(),
            "unset"
        );
        execute(true);
        assert_eq!(
            std::fs::read_to_string(directory.join("codex-keyboard"))
                .unwrap()
                .trim(),
            "0"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("claude-cursor"))
                .unwrap()
                .trim(),
            "0"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn terminal_still_runs_interactive_shell_commands() {
        let mut terminal = TerminalSession::spawn("", Path::new("/tmp")).unwrap();
        terminal.write(b"printf '\\x50\\x54\\x59\\x5f\\x52\\x45\\x41\\x44\\x59\\n'\n");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            terminal.poll();
            if terminal
                .selection_text(0, 0, i32::MAX, i32::MAX)
                .contains("PTY_READY")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "shell did not process input"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn answers_split_default_color_queries_without_moving_scrollback() {
        let (sender, receiver) = mpsc::channel();
        let mut parser = vt100::Parser::new_with_callbacks(3, 12, 20, TerminalCallbacks::default());
        parser.process(b"one\r\ntwo\r\nthree\r\nfour");
        assert!(scroll_screen(parser.screen_mut(), 1));

        sender.send(b"\x1b]10;?\x1b".to_vec()).unwrap();
        sender.send(b"\\\x1b]11;?\x07".to_vec()).unwrap();
        sender
            .send(b"\x1b]12;?\x07\x1b]11;rgb:0000/0000/0000\x07".to_vec())
            .unwrap();
        for bytes in receiver.try_iter() {
            parser.process(&bytes);
        }
        assert_eq!(parser.screen().scrollback(), 1);
        assert!(parser.screen().contents().contains("one"));

        let mut writer = Vec::new();
        write_terminal_replies(&mut writer, parser.callbacks_mut());
        let expected = b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:2828/2c2c/3434\x1b\\";
        assert_eq!(writer, expected);
        assert_eq!(parser.screen().scrollback(), 1);
        write_terminal_replies(&mut writer, parser.callbacks_mut());
        assert_eq!(writer, expected);
    }

    use std::sync::atomic::AtomicUsize;

    #[test]
    fn encodes_control_keys() {
        assert_eq!(encode_key("c", true, false, false), vec![3]);
        assert_eq!(encode_key("x", false, true, false), b"\x1bx");
    }

    #[test]
    fn preserves_all_modifier_combinations_on_navigation_editing_and_function_keys() {
        for (control, alt, shift, modifier) in [
            (false, false, true, 2),
            (false, true, false, 3),
            (false, true, true, 4),
            (true, false, false, 5),
            (true, false, true, 6),
            (true, true, false, 7),
            (true, true, true, 8),
        ] {
            for (key, final_byte) in [
                ("<LEFT>", 'D'),
                ("<RIGHT>", 'C'),
                ("<UP>", 'A'),
                ("<DOWN>", 'B'),
                ("<HOME>", 'H'),
                ("<END>", 'F'),
                ("<F1>", 'P'),
            ] {
                assert_eq!(
                    encode_key(key, control, alt, shift),
                    format!("\x1b[1;{modifier}{final_byte}").as_bytes()
                );
            }
            for (key, number) in [
                ("<INSERT>", 2),
                ("<DELETE>", 3),
                ("<PAGEUP>", 5),
                ("<PAGEDOWN>", 6),
                ("<F5>", 15),
                ("<F12>", 24),
            ] {
                assert_eq!(
                    encode_key(key, control, alt, shift),
                    format!("\x1b[{number};{modifier}~").as_bytes()
                );
            }
        }
        assert_eq!(encode_key("<LEFT>", false, false, false), b"\x1b[D");
        assert_eq!(encode_key("<DELETE>", false, false, false), b"\x1b[3~");
        assert_eq!(encode_key("<F1>", false, false, false), b"\x1bOP");
    }

    #[test]
    fn disambiguates_enter_and_control_combinations_without_changing_plain_text() {
        assert_eq!(encode_key("<ENTER>", false, false, true), b"\x1b[13;2u");
        assert_eq!(encode_key("<ENTER>", true, false, false), b"\x1b[13;5u");
        assert_eq!(encode_key("<ENTER>", false, true, false), b"\x1b\r");
        assert_eq!(encode_key("<ENTER>", false, false, false), b"\r");
        assert_eq!(encode_key("<BACKSPACE>", false, false, false), b"\x7f");
        assert_eq!(
            encode_key("<BACKSPACE>", true, false, false),
            b"\x1b[127;5u"
        );
        for (key, byte) in [
            (" ", 0),
            ("[", 27),
            ("\\", 28),
            ("]", 29),
            ("^", 30),
            ("_", 31),
        ] {
            assert_eq!(encode_key(key, true, false, false), [byte]);
        }
        assert_eq!(encode_key("r", true, false, true), b"\x1b[114;6u");
        assert_eq!(encode_key("b", false, true, true), b"\x1bB");
        assert_eq!(encode_key("]", false, true, false), b"\x1b[93;3u");
        assert_eq!(encode_key("[", false, true, false), b"\x1b[91;3u");
        assert_eq!(encode_key("o", false, true, true), b"\x1b[111;4u");
        assert_eq!(
            encode_key("한글A!", false, false, false),
            "한글A!".as_bytes()
        );
    }

    #[test]
    fn ignores_modifier_only_keys_and_encodes_backtab() {
        assert!(encode_key("\u{10}", false, false, true).is_empty());
        assert_eq!(encode_key("\u{19}", false, false, true), b"\x1b[Z");
    }

    #[test]
    fn gives_cjk_glyphs_two_terminal_columns() {
        let mut parser = vt100::Parser::new(1, 8, 0);
        parser.process("한A".as_bytes());
        let screen = parser.screen();
        assert_eq!(cell_column_span(screen.cell(0, 0)), 2);
        assert_eq!(cell_column_span(screen.cell(0, 1)), 0);
        assert_eq!(cell_column_span(screen.cell(0, 2)), 1);
    }

    #[test]
    fn scrolls_through_retained_output_and_hides_the_live_cursor() {
        let mut parser = vt100::Parser::new(3, 12, 20);
        parser.process(b"one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(parser.screen().scrollback(), 0);
        assert!(!parser.screen().contents().contains("one"));

        assert!(scroll_screen(parser.screen_mut(), 2));
        assert!(parser.screen().scrollback() > 0);
        assert!(parser.screen().contents().contains("one"));
        assert!(
            display_cells(parser.screen())
                .iter()
                .all(|cell| !cell.cursor)
        );

        assert!(scroll_screen(parser.screen_mut(), -20));
        assert_eq!(parser.screen().scrollback(), 0);
        assert!(parser.screen().contents().contains("four"));
    }

    fn wheel(rows: i32) -> TerminalWheel {
        TerminalWheel {
            rows,
            row: 1,
            column: 4,
            ..TerminalWheel::default()
        }
    }

    #[cfg(unix)]
    #[test]
    fn pty_application_receives_wheel_reports_and_shift_stays_local() {
        let mut terminal = TerminalSession::spawn("", Path::new("/tmp")).unwrap();
        terminal.write(b"stty -echo -icanon min 1 time 0; printf '\\033[?1049h\\033[?1000h\\033[?1006h'; dd bs=1 count=10 2>/dev/null | od -An -tx1; stty sane\r");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !terminal.output.frame.alternate
            || terminal.output.frame.mouse_encoding != vt100::MouseProtocolEncoding::Sgr
        {
            terminal.poll();
            assert!(
                std::time::Instant::now() < deadline,
                "mock TUI did not enable mouse capture"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        terminal.scroll_scrollback(TerminalWheel {
            shift: true,
            ..wheel(1)
        });
        // If Shift had injected bytes, dd would consume them before this report.
        terminal.scroll_scrollback(wheel(1));
        while !terminal
            .selection_text(0, 0, i32::MAX, i32::MAX)
            .contains("1b 5b 3c 36 34 3b 35 3b 32 4d")
        {
            terminal.poll();
            assert!(
                std::time::Instant::now() < deadline,
                "PTY received incorrect wheel bytes: {}",
                terminal.selection_text(0, 0, i32::MAX, i32::MAX)
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn wheel_uses_requested_mouse_protocol_in_both_screen_buffers() {
        let mut parser = vt100::Parser::new(3, 12, 20);
        for enter in [b"".as_slice(), b"\x1b[?1049h".as_slice()] {
            parser.process(enter);
            assert_eq!(mouse_scroll_input(parser.screen(), wheel(3)), None);
            for mode in [1000, 1002, 1003] {
                parser.process(format!("\x1b[?{mode}h\x1b[?1006h").as_bytes());
                assert_eq!(
                    mouse_scroll_input(parser.screen(), wheel(3)),
                    Some(b"\x1b[<64;5;2M".repeat(3))
                );
                assert_eq!(
                    mouse_scroll_input(parser.screen(), wheel(-2)),
                    Some(b"\x1b[<65;5;2M".repeat(2))
                );
                assert_eq!(mouse_scroll_input(parser.screen(), wheel(0)), None);
                parser.process(format!("\x1b[?{mode}l").as_bytes());
                assert_eq!(mouse_scroll_input(parser.screen(), wheel(3)), None);
            }
            // X10 asks for button presses only, not wheel reports.
            parser.process(b"\x1b[?9h");
            assert_eq!(mouse_scroll_input(parser.screen(), wheel(3)), None);
            parser.process(b"\x1b[?9l");
        }
    }

    #[test]
    fn wheel_encodes_legacy_and_utf8_mouse_coordinates_without_overflow() {
        let mut parser = vt100::Parser::new(300, 400, 20);
        parser.process(b"\x1b[?1000h");
        assert_eq!(
            mouse_scroll_input(parser.screen(), wheel(1)),
            Some(b"\x1b[M`%\"".to_vec())
        );
        let large = TerminalWheel {
            column: 299,
            ..wheel(-1)
        };
        assert_eq!(mouse_scroll_input(parser.screen(), large), None);
        parser.process(b"\x1b[?1005h");
        assert_eq!(
            mouse_scroll_input(parser.screen(), large),
            Some("\x1b[Ma\u{14c}\"".as_bytes().to_vec())
        );
        parser.process(b"\x1b[?1006h");
        assert_eq!(
            mouse_scroll_input(parser.screen(), large),
            Some(b"\x1b[<65;300;2M".to_vec())
        );
        let modifiers = TerminalWheel {
            control: true,
            alt: true,
            ..wheel(1)
        };
        assert_eq!(
            mouse_scroll_input(parser.screen(), modifiers),
            Some(b"\x1b[<88;5;2M".to_vec())
        );
        let outside = TerminalWheel {
            row: -2,
            column: i32::MAX,
            ..wheel(1)
        };
        assert_eq!(
            mouse_scroll_input(parser.screen(), outside),
            Some(b"\x1b[<64;400;1M".to_vec())
        );
        assert_eq!(
            mouse_scroll_input(parser.screen(), wheel(i32::MIN)),
            Some(b"\x1b[<65;5;2M".repeat(100))
        );
    }

    #[test]
    fn shift_wheel_reads_retained_output_without_reporting_input() {
        let mut parser = vt100::Parser::new(3, 12, 20);
        parser.process(b"one\r\ntwo\r\nthree\r\nfour\x1b[?1000h\x1b[?1006h");
        let local = TerminalWheel {
            shift: true,
            ..wheel(1)
        };
        assert_eq!(mouse_scroll_input(parser.screen(), local), None);
        assert!(scroll_screen(parser.screen_mut(), local.rows));
        assert!(parser.screen().contents().contains("one"));
        let report = mouse_scroll_input(parser.screen(), wheel(-1)).unwrap();
        let mut writer = Vec::new();
        assert!(report_mouse_scroll(
            parser.screen_mut(),
            &mut writer,
            &report
        ));
        assert_eq!(writer, b"\x1b[<65;5;2M");
        assert_eq!(parser.screen().scrollback(), 0);
        assert!(parser.screen().contents().contains("four"));
    }

    #[test]
    fn failed_mouse_report_keeps_local_history_position() {
        let mut parser = vt100::Parser::new(3, 12, 20);
        parser.process(b"one\r\ntwo\r\nthree\r\nfour\x1b[?1000h\x1b[?1006h");
        assert!(scroll_screen(parser.screen_mut(), 1));
        let report = mouse_scroll_input(parser.screen(), wheel(-1)).unwrap();
        assert!(!report_mouse_scroll(
            parser.screen_mut(),
            &mut &mut [0_u8; 0][..],
            &report
        ));
        assert_eq!(parser.screen().scrollback(), 1);
    }

    #[test]
    fn alternate_screen_history_is_bounded_isolated_and_reset_on_new_session() {
        let mut parser = vt100::Parser::new(3, 12, 2);
        parser.process(b"shell1\r\nshell2\r\nshell3\r\nshell4");
        parser.process(b"\x1b[?1049hone\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
        // Inspect displaced output without injecting arrow keys.
        assert_eq!(mouse_scroll_input(parser.screen(), wheel(3)), None);
        assert!(scroll_screen(parser.screen_mut(), 100));
        assert_eq!(parser.screen().scrollback(), 2);
        assert_eq!(parser.screen().contents(), "two\nthree\nfour");
        parser.process(b"\r\nseven");
        assert_eq!(parser.screen().scrollback(), 2);
        assert_eq!(parser.screen().contents(), "three\nfour\nfive");
        assert!(
            display_cells(parser.screen())
                .iter()
                .all(|cell| !cell.cursor)
        );
        parser.process(b"\x1b[?1049l");
        assert_eq!(parser.screen().scrollback(), 0);
        assert!(scroll_screen(parser.screen_mut(), 1));
        assert!(parser.screen().contents().contains("shell1"));
        parser.process(b"\x1b[?1049hnew session");
        assert_eq!(parser.screen().scrollback(), 0);
        assert!(!scroll_screen(parser.screen_mut(), 100));
        assert_eq!(parser.screen().contents(), "new session");
    }

    #[test]
    fn inline_tui_top_scroll_region_retains_scrolled_answer_rows() {
        let mut parser = vt100::Parser::new(4, 12, 20);
        parser.process(b"one\r\ntwo\r\nthree\r\nprompt");
        // An inline TUI scrolls only the transcript, leaving its prompt at
        // the bottom. The displaced top row must remain available to wheel.
        parser.process(b"\x1b[1;3r\x1b[S\x1b[r");
        assert!(!parser.screen().alternate_screen());
        assert!(!parser.screen().contents().contains("one"));
        assert!(scroll_screen(parser.screen_mut(), 1));
        assert!(parser.screen().contents().contains("one"));
    }

    #[test]
    fn copies_forward_and_reverse_terminal_cell_selections() {
        let mut parser = vt100::Parser::new(3, 12, 0);
        parser.process(b"alpha\r\nbeta\r\ngamma");

        assert_eq!(selection_text(parser.screen(), 0, 1, 0, 3), "lph");
        assert_eq!(selection_text(parser.screen(), 1, 1, 2, 2), "eta\ngam");
        assert_eq!(selection_text(parser.screen(), 2, 2, 1, 1), "eta\ngam");
    }

    #[test]
    fn omits_blank_cells_but_preserves_grid_positions_and_styled_spaces() {
        let mut parser = vt100::Parser::new(3, 8, 0);
        parser.process(b"A\x1b[2;3H\x1b[41m \x1b[0m");

        let cells = display_cells(parser.screen());
        assert!(cells.len() < 3 * 8);
        assert!(
            cells
                .iter()
                .any(|cell| { cell.row == 0 && cell.column == 0 && cell.glyph == "A" })
        );
        assert!(cells.iter().any(|cell| {
            cell.row == 1
                && cell.column == 2
                && cell.glyph == " "
                && cell.background == ANSI_COLORS[1]
        }));
    }

    #[test]
    fn uses_orca_colors_for_plain_bold_and_truecolor_output() {
        let mut parser = vt100::Parser::new(1, 20, 0);
        parser.process(b"A\x1b[1;34mB\x1b[38;2;10;20;30mC");
        let cells = display_cells(parser.screen());
        let plain = cells.iter().find(|cell| cell.glyph == "A").unwrap();
        assert_eq!(plain.foreground, [255, 255, 255]);
        assert_eq!(plain.background, [0x28, 0x2c, 0x34]);
        assert_eq!(
            cells
                .iter()
                .find(|cell| cell.glyph == "B")
                .unwrap()
                .foreground,
            [0x7a, 0xa6, 0xda]
        );
        assert_eq!(
            cells
                .iter()
                .find(|cell| cell.glyph == "C")
                .unwrap()
                .foreground,
            [10, 20, 30]
        );
    }

    #[test]
    fn coalesces_output_wakes_until_the_ui_drains_the_terminal() {
        let signal = OutputSignal::default();
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed_wakes = wakes.clone();
        signal.set_waker(Arc::new(move || {
            observed_wakes.fetch_add(1, AtomicOrdering::Relaxed);
        }));

        signal.notify();
        signal.notify();
        assert_eq!(wakes.load(AtomicOrdering::Relaxed), 1);

        signal.clear_pending();
        signal.notify();
        assert_eq!(wakes.load(AtomicOrdering::Relaxed), 2);
    }

    #[test]
    fn first_resize_is_immediate_and_continuous_resizes_are_rate_limited() {
        let mut throttle = ResizeThrottle::default();
        assert!(
            throttle.observe(),
            "first change must not wait for stable polls"
        );
        throttle.retry_later();
        assert!(!throttle.observe());
        assert!(
            throttle.observe(),
            "continuous dragging must still deliver sizes"
        );
        throttle.retry_later();
        assert!(!throttle.observe());
        assert!(throttle.observe());
    }

    #[test]
    fn in_flight_resize_returns_to_original_size_and_coalesces_intermediate_sizes() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let mut resize = WindowsPtyResize::start(move |rows, columns| {
            seen_tx.send((rows, columns)).unwrap();
            finish_rx.recv().unwrap()
        })
        .unwrap();
        for _ in 0..3 {
            resize.apply(40, 120);
        }
        assert_eq!(
            seen_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            (40, 120)
        );
        resize.apply(50, 140);
        for _ in 0..3 {
            resize.apply(24, 80);
        }
        assert!(
            seen_rx.try_recv().is_err(),
            "only one WSL request may be in flight"
        );
        finish_tx.send(true).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            resize.apply(24, 80);
            if let Ok(size) = seen_rx.try_recv() {
                assert_eq!(size, (24, 80));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "original size was never restored"
            );
            thread::yield_now();
        }
        finish_tx.send(true).unwrap();
        while resize.in_flight.is_some() {
            resize.apply(24, 80);
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        for _ in 0..10 {
            resize.apply(24, 80);
        }
        assert!(
            seen_rx.try_recv().is_err(),
            "an unchanged terminal must not spawn more WSL work"
        );
        assert_eq!(resize.applied, (24, 80));
    }

    #[test]
    fn failed_resize_retries_the_latest_size() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let mut resize = WindowsPtyResize::start(move |rows, columns| {
            seen_tx.send((rows, columns)).unwrap();
            finish_rx.recv().unwrap()
        })
        .unwrap();
        for _ in 0..3 {
            resize.apply(40, 120);
        }
        assert_eq!(
            seen_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            (40, 120)
        );
        finish_tx.send(false).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            resize.apply(50, 140);
            if let Ok(size) = seen_rx.try_recv() {
                assert_eq!(size, (50, 140));
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        finish_tx.send(true).unwrap();
        while resize.in_flight.is_some() {
            resize.apply(50, 140);
            thread::yield_now();
            assert!(std::time::Instant::now() < deadline);
        }
        assert_eq!(resize.applied, (50, 140));
    }

    #[test]
    fn idle_resize_dispatches_immediately_without_extra_work() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let mut resize = WindowsPtyResize::start(move |rows, columns| {
            seen_tx.send((rows, columns)).unwrap();
            true
        })
        .unwrap();
        resize.apply(24, 80);
        assert!(seen_rx.try_recv().is_err());
        resize.apply(40, 120);
        assert_eq!(
            seen_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            (40, 120)
        );
    }
    #[test]
    fn hidden_cursor_keeps_ime_position_without_painting_a_cursor() {
        let mut parser = vt100::Parser::new(24, 80, 10);
        parser.process(b"\x1b[10;18H\x1b[?25l");
        assert_eq!(input_cursor_position(parser.screen()), Some((9, 17)));
        assert!(
            display_cells(parser.screen())
                .iter()
                .all(|cell| !cell.cursor)
        );
        parser.process(b"\x1b[?25h");
        assert_eq!(input_cursor_position(parser.screen()), Some((9, 17)));
        assert!(
            display_cells(parser.screen())
                .iter()
                .any(|cell| cell.cursor)
        );
        parser.process(b"\x1b[24;1H\r\n\r\n");
        parser.screen_mut().set_scrollback(1);
        assert_eq!(input_cursor_position(parser.screen()), None);
    }
    #[cfg(unix)]
    #[test]
    fn docking_a_live_tui_resizes_without_input_and_accepts_the_first_key() {
        use std::time::{Duration, Instant};
        let mut terminal = TerminalSession::spawn_pty(Path::new("/tmp")).unwrap();
        // This owned fixture behaves like a full-screen CLI, with a resize
        // handler and raw input. It never starts Codex or uses user sessions.
        let python = r#"import os,signal,sys,tty,select
tty.setraw(0)
def draw(*args):
 s=os.get_terminal_size(0)
 sys.stdout.write('\x1b[2J\x1b[HGRID %dx%d' % (s.lines,s.columns));sys.stdout.flush()
signal.signal(signal.SIGWINCH,draw)
draw()
while True:
 c=os.read(0,1)
 if not c: break
 sys.stdout.write('\x1b[2;1HKEY-'+c.decode()+'\x1b[6n');sys.stdout.flush()
 reply=b''
 while not reply.endswith(b'R'):
  if not select.select([0],[],[],1)[0]: raise RuntimeError('cursor reply timed out')
  reply+=os.read(0,1)
 if reply!=b'\x1b[2;6R': raise RuntimeError('wrong cursor reply: '+repr(reply))
 sys.stdout.write('\x1b[3;1HACK-'+c.decode());sys.stdout.flush()
"#;
        terminal.write(format!("python3 -u -c {}\r", shell_quote(python)).as_bytes());
        let wait_for = |terminal: &mut TerminalSession, text: &str, timeout: Duration| {
            let deadline = Instant::now() + timeout;
            loop {
                terminal.poll();
                if terminal
                    .selection_text(0, 0, i32::MAX, i32::MAX)
                    .contains(text)
                {
                    break;
                }
                assert!(Instant::now() < deadline, "TUI never produced {text}");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        wait_for(&mut terminal, "GRID 24x100", Duration::from_secs(5));
        let full = crate::terminal_layout::grid_size(944.0, 776.0, 14).unwrap();
        terminal.resize(full.0, full.1);
        wait_for(&mut terminal, "GRID 46x118", Duration::from_secs(1));
        let mut groups = crate::tabs::TabGroups::default();
        groups.add(1, 0);
        groups.add(2, 0);
        assert!(groups.dock_into(2, 0, crate::tabs::Dock::Bottom));
        let (_, rect) = groups
            .layout()
            .0
            .into_iter()
            .find(|(id, _)| *id == groups.group_of(2).unwrap())
            .unwrap();
        let split =
            crate::terminal_layout::grid_size(944.0 * rect.width, 776.0 * rect.height, 14).unwrap();
        terminal.resize(split.0, split.1);
        wait_for(&mut terminal, "GRID 22x118", Duration::from_secs(1));
        assert_eq!(terminal.size(), split);
        for letter in [b'x', b'y', b'z'] {
            terminal.write(&[letter]);
            wait_for(
                &mut terminal,
                &format!("ACK-{}", char::from(letter)),
                Duration::from_secs(2),
            );
        }
    }
    #[test]
    fn answers_cursor_and_size_queries_at_the_current_grid_without_sending_user_input() {
        let mut parser =
            vt100::Parser::new_with_callbacks(24, 80, 10, TerminalCallbacks::default());
        parser.process(b"\x1b[13;8H\x1b[?25l\x1b[5n\x1b[6");
        parser.process(b"n\x1b[?6n\x1b[18t");
        let mut replies = Vec::new();
        write_terminal_replies(&mut replies, parser.callbacks_mut());
        assert_eq!(replies, b"\x1b[0n\x1b[13;8R\x1b[?13;8R\x1b[8;24;80t");
        assert_eq!(parser.screen().cursor_position(), (12, 7));
        parser.screen_mut().set_size(8, 20);
        parser.process(b"\x1b[6n\x1b[18t\x1b[0n\x1b[?5n\x1b[6;1n");
        replies.clear();
        write_terminal_replies(&mut replies, parser.callbacks_mut());
        assert_eq!(replies, b"\x1b[8;8R\x1b[8;8;20t");
        write_terminal_replies(&mut replies, parser.callbacks_mut());
        assert_eq!(replies, b"\x1b[8;8R\x1b[8;8;20t");
    }

    #[test]
    fn cursor_replies_respect_origin_mode_and_pending_wrap() {
        let mut parser =
            vt100::Parser::new_with_callbacks(24, 80, 10, TerminalCallbacks::default());
        parser.process(b"\x1b[4;20r\x1b[?6h\x1b[2;9H\x1b[6n");
        let mut replies = Vec::new();
        write_terminal_replies(&mut replies, parser.callbacks_mut());
        assert_eq!(replies, b"\x1b[2;9R");
        parser.process(b"\x1b[?6l");
        parser.screen_mut().set_size(2, 3);
        parser.process(b"\x1b[Habc\x1b[6n");
        replies.clear();
        write_terminal_replies(&mut replies, parser.callbacks_mut());
        assert_eq!(replies, b"\x1b[1;3R");
    }
}
