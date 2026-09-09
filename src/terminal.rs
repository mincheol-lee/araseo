use anyhow::{Context, Result};
#[cfg(not(target_os = "windows"))]
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
static NEXT_WINDOWS_TERMINAL_ID: AtomicU32 = AtomicU32::new(1);

#[cfg(any(target_os = "windows", test))]
#[derive(Default)]
struct ResizeDebouncer {
    requested: Option<(u16, u16)>,
    stable_polls: u8,
}

#[cfg(any(target_os = "windows", test))]
impl ResizeDebouncer {
    fn observe(&mut self, rows: u16, columns: u16) -> bool {
        let requested = (rows, columns);
        if self.requested != Some(requested) {
            self.requested = Some(requested);
            self.stable_polls = 0;
            return false;
        }
        self.stable_polls = self.stable_polls.saturating_add(1);
        self.stable_polls >= 2
    }

    fn retry_later(&mut self) {
        self.stable_polls = 0;
    }
}

#[cfg(target_os = "windows")]
struct WindowsPtyResize {
    request_sender: mpsc::SyncSender<(u16, u16)>,
    result_receiver: mpsc::Receiver<((u16, u16), bool)>,
    in_flight: Option<(u16, u16)>,
    debouncer: ResizeDebouncer,
}

#[cfg(target_os = "windows")]
impl WindowsPtyResize {
    fn new(distro: String, tty_path_file: String) -> Self {
        let (request_sender, request_receiver) = mpsc::sync_channel(1);
        let (result_sender, result_receiver) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(size @ (rows, columns)) = request_receiver.recv() {
                let resized = resize_windows_pty(&distro, &tty_path_file, rows, columns);
                if result_sender.send((size, resized)).is_err() {
                    break;
                }
            }
        });

        Self {
            request_sender,
            result_receiver,
            in_flight: None,
            debouncer: ResizeDebouncer::default(),
        }
    }

    fn apply(&mut self, rows: u16, columns: u16) -> Option<(u16, u16)> {
        if let Ok((completed, resized)) = self.result_receiver.try_recv() {
            self.in_flight = None;
            if resized {
                return Some(completed);
            }
            self.debouncer.retry_later();
        }

        if self.in_flight.is_some() || !self.debouncer.observe(rows, columns) {
            return None;
        }

        let requested = (rows, columns);
        if self.request_sender.try_send(requested).is_ok() {
            self.in_flight = Some(requested);
        } else {
            self.debouncer.retry_later();
        }
        None
    }
}

#[cfg(target_os = "windows")]
fn resize_windows_pty(distro: &str, tty_path_file: &str, rows: u16, columns: u16) -> bool {
    let mut command = std::process::Command::new(r"C:\Windows\System32\wsl.exe");
    command.creation_flags(CREATE_NO_WINDOW);
    command.args([
        "-d",
        distro,
        "--exec",
        "/bin/sh",
        "-c",
        "araseo_tty=$(cat \"$1\") && exec /usr/bin/stty -F \"$araseo_tty\" rows \"$2\" cols \"$3\"",
        "araseo-resize",
    ]);
    command
        .arg(tty_path_file)
        .arg(rows.to_string())
        .arg(columns.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    command.status().is_ok_and(|status| status.success())
}

const DEFAULT_FOREGROUND: [u8; 3] = [0xff, 0xff, 0xff];
const DEFAULT_BACKGROUND: [u8; 3] = [0x28, 0x2c, 0x34];

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

pub struct TerminalSession {
    #[cfg(not(target_os = "windows"))]
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    receiver: mpsc::Receiver<Vec<u8>>,
    output_signal: Arc<OutputSignal>,
    parser: vt100::Parser,
    _pty_child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    _pipe_child: Option<std::process::Child>,
    #[cfg(target_os = "windows")]
    windows_resize: WindowsPtyResize,
}

impl TerminalSession {
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
        let terminal_id = NEXT_WINDOWS_TERMINAL_ID.fetch_add(1, Ordering::Relaxed);
        let tty_path_file = format!("/tmp/araseo-pty-{}-{terminal_id}", std::process::id());
        let shell_command = format!(
            "stty rows 24 cols 80; tty > {tty_path_file}; \
             /usr/bin/env TERM=xterm-256color COLORTERM=truecolor /bin/bash --login -i; \
             araseo_status=$?; rm -f {tty_path_file}; exit $araseo_status"
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

        let mut child = command.spawn().context("failed to start WSL shell")?;
        let stdout = child.stdout.take().context("WSL stdout is unavailable")?;
        let stderr = child.stderr.take().context("WSL stderr is unavailable")?;
        let stdin = child.stdin.take().context("WSL stdin is unavailable")?;

        // Backpressure bounds unread output to roughly 512 KiB per terminal.
        let (sender, receiver) = mpsc::sync_channel(64);
        let output_signal = Arc::new(OutputSignal::default());
        spawn_reader(stdout, sender.clone(), output_signal.clone());
        spawn_reader(stderr, sender, output_signal.clone());

        Ok(Self {
            writer: Arc::new(Mutex::new(Box::new(stdin))),
            receiver,
            output_signal,
            // util-linux `script` allocates an 80x24 Unix PTY by default.
            // Keep the VT parser at the same size so full-screen TUIs such as
            // Codex do not wrap every row into a narrow UI-sized buffer.
            parser: vt100::Parser::new(24, 80, 10_000),
            _pty_child: None,
            _pipe_child: Some(child),
            windows_resize: WindowsPtyResize::new(distro.to_string(), tty_path_file),
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
        command.args(["--login", "-i"]);
        command.cwd(linux_root);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let child = pair
            .slave
            .spawn_command(command)
            .context("failed to start WSL shell")?;
        let reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let (sender, receiver) = mpsc::sync_channel(64);
        let output_signal = Arc::new(OutputSignal::default());
        spawn_reader(reader, sender, output_signal.clone());

        Ok(Self {
            master: pair.master,
            writer,
            receiver,
            output_signal,
            parser: vt100::Parser::new(24, 100, 10_000),
            _pty_child: Some(child),
            _pipe_child: None,
        })
    }

    pub fn poll(&mut self) -> bool {
        poll_output(&self.receiver, &self.output_signal, &mut self.parser)
    }

    pub fn set_output_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.output_signal.set_waker(Arc::new(waker));
    }

    pub fn cells(&self) -> Vec<DisplayCell> {
        display_cells(self.parser.screen())
    }

    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    pub fn cursor_row(&self) -> i32 {
        if self.parser.screen().hide_cursor() {
            -1
        } else {
            self.parser.screen().cursor_position().0.into()
        }
    }

    pub fn cursor_column(&self) -> i32 {
        if self.parser.screen().hide_cursor() {
            -1
        } else {
            self.parser.screen().cursor_position().1.into()
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> bool {
        if self.parser.screen().size() == (rows, cols) {
            return false;
        }

        #[cfg(target_os = "windows")]
        let Some((rows, cols)) = self.windows_resize.apply(rows, cols) else {
            return false;
        };

        #[cfg(not(target_os = "windows"))]
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.parser.screen_mut().set_size(rows, cols);
        true
    }
}

// Bound one UI callback even when a command continuously produces output.
// Chunk boundaries are not VT boundaries: the parser retains partial escapes.
fn poll_output(
    receiver: &mpsc::Receiver<Vec<u8>>,
    signal: &OutputSignal,
    parser: &mut vt100::Parser,
) -> bool {
    let mut processed = 0;
    let started = std::time::Instant::now();
    loop {
        let bytes = match receiver.try_recv() {
            Ok(bytes) => bytes,
            Err(_) => {
                signal.clear_pending();
                // Close the race between draining the channel and clearing
                // the signal, without dropping output or losing a wakeup.
                match receiver.try_recv() {
                    Ok(bytes) => bytes,
                    Err(_) => return processed > 0,
                }
            }
        };
        parser.process(&bytes);
        processed += bytes.len();
        if processed >= 64 * 1024 || started.elapsed() >= std::time::Duration::from_millis(4) {
            signal.clear_pending();
            signal.notify();
            return true;
        }
    }
}

fn display_cells(screen: &vt100::Screen) -> Vec<DisplayCell> {
    let (rows, columns) = screen.size();
    let cursor_position = (!screen.hide_cursor()).then(|| screen.cursor_position());
    let mut cells = Vec::new();
    for row in 0..rows {
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
    }
    cells
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
    sender: mpsc::SyncSender<Vec<u8>>,
    output_signal: Arc<OutputSignal>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                    output_signal.notify();
                }
            }
        }
    });
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

pub fn encode_key(text: &str, control: bool, alt: bool, _shift: bool) -> Vec<u8> {
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
    ) {
        return Vec::new();
    }
    let mut bytes = if control && text.len() == 1 {
        let byte = text.as_bytes()[0].to_ascii_lowercase();
        if byte.is_ascii_lowercase() {
            vec![byte - b'a' + 1]
        } else {
            text.as_bytes().to_vec()
        }
    } else {
        match text {
            "<UP>" => b"\x1b[A".to_vec(),
            "<DOWN>" => b"\x1b[B".to_vec(),
            "<LEFT>" => b"\x1b[D".to_vec(),
            "<RIGHT>" => b"\x1b[C".to_vec(),
            "<HOME>" => b"\x1b[H".to_vec(),
            "<END>" => b"\x1b[F".to_vec(),
            "<PAGEUP>" => b"\x1b[5~".to_vec(),
            "<PAGEDOWN>" => b"\x1b[6~".to_vec(),
            "<DELETE>" => b"\x1b[3~".to_vec(),
            "<BACKSPACE>" => vec![0x7f],
            "<ENTER>" | "\n" | "\r" => b"\r".to_vec(),
            "<BACKTAB>" | "\u{19}" => b"\x1b[Z".to_vec(),
            "<TAB>" => b"\t".to_vec(),
            "<ESCAPE>" => vec![0x1b],
            value => value.as_bytes().to_vec(),
        }
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
    fn busy_terminal_yields_and_reschedules_without_losing_output() {
        let (sender, receiver) = mpsc::channel();
        let signal = OutputSignal::default();
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = wakes.clone();
        signal.set_waker(Arc::new(move || {
            count.fetch_add(1, AtomicOrdering::SeqCst);
        }));
        for _ in 0..32 {
            sender.send(vec![b'x'; 8192]).unwrap();
        }
        // Include an escape and UTF-8 character split across chunks.
        sender.send(b"\x1b[2J\x1b[H\x1b[3".to_vec()).unwrap();
        sender
            .send([b"1m".as_slice(), &"한".as_bytes()[..1]].concat())
            .unwrap();
        sender.send("한".as_bytes()[1..].to_vec()).unwrap();
        let mut parser = vt100::Parser::new(24, 80, 0);
        assert!(poll_output(&receiver, &signal, &mut parser));
        assert!(wakes.load(AtomicOrdering::SeqCst) > 0);
        assert!(!parser.screen().contents().contains('한'));
        let mut polls = 1;
        while poll_output(&receiver, &signal, &mut parser) {
            polls += 1;
            assert!(polls < 100);
        }
        assert!(polls > 1);
        assert_eq!(parser.screen().contents(), "한");
        assert_eq!(
            parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        assert!(!signal.pending.load(AtomicOrdering::Acquire));
    }
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn encodes_control_keys() {
        assert_eq!(encode_key("c", true, false, false), vec![3]);
        assert_eq!(encode_key("x", false, true, false), b"\x1bx");
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
        assert_eq!(cells.iter().find(|cell| cell.glyph == "B").unwrap().foreground, [0x7a, 0xa6, 0xda]);
        assert_eq!(cells.iter().find(|cell| cell.glyph == "C").unwrap().foreground, [10, 20, 30]);
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
    fn waits_for_a_stable_terminal_size_before_resizing() {
        let mut debouncer = ResizeDebouncer::default();

        assert!(!debouncer.observe(40, 120));
        assert!(!debouncer.observe(40, 120));
        assert!(debouncer.observe(40, 120));

        assert!(!debouncer.observe(50, 160));
        assert!(!debouncer.observe(50, 160));
        debouncer.retry_later();
        assert!(!debouncer.observe(50, 160));
        assert!(debouncer.observe(50, 160));
    }
}
