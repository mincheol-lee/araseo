use anyhow::{Context, Result};
#[cfg(not(target_os = "windows"))]
use portable_pty::{CommandBuilder, native_pty_system};
use portable_pty::{MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::Path;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct DisplayCell {
    pub glyph: String,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub bold: bool,
    pub cursor: bool,
    pub column_span: i32,
}

pub struct TerminalSession {
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    receiver: mpsc::Receiver<Vec<u8>>,
    parser: vt100::Parser,
    _pty_child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    _pipe_child: Option<std::process::Child>,
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
        let mut command = std::process::Command::new(r"C:\Windows\System32\wsl.exe");
        command.creation_flags(CREATE_NO_WINDOW);
        command
            .args(["-d", distro, "--cd"])
            .arg(linux_root)
            .args([
                "--exec",
                "/usr/bin/script",
                "-qfec",
                "stty rows 24 cols 80; exec /usr/bin/env TERM=xterm-256color COLORTERM=truecolor /bin/bash --login -i",
                "/dev/null",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().context("failed to start WSL shell")?;
        let stdout = child.stdout.take().context("WSL stdout is unavailable")?;
        let stderr = child.stderr.take().context("WSL stderr is unavailable")?;
        let stdin = child.stdin.take().context("WSL stdin is unavailable")?;

        let (sender, receiver) = mpsc::channel();
        spawn_reader(stdout, sender.clone());
        spawn_reader(stderr, sender);

        Ok(Self {
            master: None,
            writer: Arc::new(Mutex::new(Box::new(stdin))),
            receiver,
            // util-linux `script` allocates an 80x24 Unix PTY by default.
            // Keep the VT parser at the same size so full-screen TUIs such as
            // Codex do not wrap every row into a narrow UI-sized buffer.
            parser: vt100::Parser::new(24, 80, 10_000),
            _pty_child: None,
            _pipe_child: Some(child),
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
        let (sender, receiver) = mpsc::channel();
        spawn_reader(reader, sender);

        Ok(Self {
            master: Some(pair.master),
            writer,
            receiver,
            parser: vt100::Parser::new(24, 100, 10_000),
            _pty_child: Some(child),
            _pipe_child: None,
        })
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(bytes) = self.receiver.try_recv() {
            self.parser.process(&bytes);
            changed = true;
        }
        changed
    }

    pub fn cells(&self) -> Vec<DisplayCell> {
        let screen = self.parser.screen();
        let (rows, columns) = screen.size();
        let cursor_position = (!screen.hide_cursor()).then(|| screen.cursor_position());
        let mut cells = Vec::with_capacity(rows as usize * columns as usize);
        for row in 0..rows {
            for column in 0..columns {
                let cell = screen.cell(row, column);
                let mut foreground = terminal_color(
                    cell.map(vt100::Cell::fgcolor).unwrap_or_default(),
                    [0xd8, 0xde, 0xe9],
                );
                let mut background = terminal_color(
                    cell.map(vt100::Cell::bgcolor).unwrap_or_default(),
                    [0x11, 0x13, 0x18],
                );
                if cell.is_some_and(vt100::Cell::inverse) {
                    std::mem::swap(&mut foreground, &mut background);
                }
                if cell.is_some_and(vt100::Cell::dim) {
                    foreground = foreground.map(|channel| channel.saturating_mul(2) / 3);
                }
                cells.push(DisplayCell {
                    glyph: cell
                        .map(vt100::Cell::contents)
                        .filter(|contents| !contents.is_empty())
                        .unwrap_or(" ")
                        .to_string(),
                    foreground,
                    background,
                    bold: cell.is_some_and(vt100::Cell::bold),
                    cursor: cursor_position == Some((row, column)),
                    column_span: cell_column_span(cell),
                });
            }
        }
        cells
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
        // The Windows backend uses a pipe to a fixed 80x24 Unix PTY created by
        // `script`; resizing only the parser would desynchronize the two ends.
        if self.master.is_none() {
            return false;
        }
        if self.parser.screen().size() == (rows, cols) {
            return false;
        }
        if let Some(master) = self.master.as_ref() {
            let _ = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        self.parser.screen_mut().set_size(rows, cols);
        true
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

fn spawn_reader(mut reader: impl Read + Send + 'static, sender: mpsc::Sender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
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

const ANSI_COLORS: [[u8; 3]; 16] = [
    [0x1b, 0x1d, 0x23],
    [0xe0, 0x6c, 0x75],
    [0x98, 0xc3, 0x79],
    [0xe5, 0xc0, 0x7b],
    [0x61, 0xaf, 0xef],
    [0xc6, 0x78, 0xdd],
    [0x56, 0xb6, 0xc2],
    [0xd8, 0xde, 0xe9],
    [0x5c, 0x63, 0x70],
    [0xff, 0x7b, 0x86],
    [0xb3, 0xe3, 0x8c],
    [0xff, 0xd6, 0x8a],
    [0x82, 0xc7, 0xff],
    [0xdc, 0x8c, 0xf0],
    [0x7f, 0xd9, 0xe5],
    [0xff, 0xff, 0xff],
];

pub fn encode_key(text: &str, control: bool, alt: bool, _shift: bool) -> Vec<u8> {
    if matches!(
        text,
        "\u{10}" | "\u{11}" | "\u{12}" | "\u{13}" | "\u{14}" | "\u{15}" | "\u{16}" | "\u{17}" | "\u{18}"
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
}
