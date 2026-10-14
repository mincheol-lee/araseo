//! Lossless VT processing independent of the UI. Only presentation frames coalesce.
use super::{
    DisplayCell, OutputSignal, TerminalCallbacks, TerminalWheel, display_row,
    input_cursor_position, mouse_scroll_input, report_mouse_scroll, scroll_screen,
    write_terminal_replies,
};
use crate::terminal_input::TerminalInput;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const MAX_WHEELS: usize = 128;

pub struct FrameRow {
    raw: Vec<Option<vt100::Cell>>,
    wrapped: bool,
    painted_cursor: Option<u16>,
    pub cells: Vec<DisplayCell>,
}

pub struct TerminalFrame {
    pub size: (u16, u16),
    pub cursor: Option<(u16, u16)>,
    pub rows: Vec<Arc<FrameRow>>,
    pub scrollback: usize,
    pub alternate: bool,
    pub mouse_encoding: vt100::MouseProtocolEncoding,
    mouse_mode: vt100::MouseProtocolMode,
    created: Instant,
}

impl TerminalFrame {
    fn capture(screen: &vt100::Screen, previous: Option<&Self>) -> Self {
        let size = screen.size();
        let cursor = input_cursor_position(screen);
        let painted_cursor = (!screen.hide_cursor()).then_some(cursor).flatten();
        let rows = (0..size.0)
            .map(|row| {
                let row_cursor = painted_cursor.filter(|(r, _)| *r == row).map(|(_, c)| c);
                let wrapped = screen.row_wrapped(row);
                if let Some(old) = previous.and_then(|frame| frame.rows.get(usize::from(row)))
                    && old.raw.len() == usize::from(size.1)
                    && old.wrapped == wrapped
                    && old.painted_cursor == row_cursor
                    && old
                        .raw
                        .iter()
                        .enumerate()
                        .all(|(column, cell)| screen.cell(row, column as u16) == cell.as_ref())
                {
                    return old.clone();
                }
                Arc::new(FrameRow {
                    raw: (0..size.1)
                        .map(|col| screen.cell(row, col).cloned())
                        .collect(),
                    wrapped,
                    painted_cursor: row_cursor,
                    cells: display_row(screen, row),
                })
            })
            .collect();
        Self {
            size,
            cursor,
            rows,
            scrollback: screen.scrollback(),
            alternate: screen.alternate_screen(),
            mouse_encoding: screen.mouse_protocol_encoding(),
            mouse_mode: screen.mouse_protocol_mode(),
            created: Instant::now(),
        }
    }

    fn same_view(&self, other: &Self) -> bool {
        self.size == other.size
            && self.cursor == other.cursor
            && self.scrollback == other.scrollback
            && self.rows.len() == other.rows.len()
            && self
                .rows
                .iter()
                .zip(&other.rows)
                .all(|(a, b)| Arc::ptr_eq(a, b))
            && self.alternate == other.alternate
            && self.mouse_encoding == other.mouse_encoding
            && self.mouse_mode == other.mouse_mode
    }

    pub fn selection_text(
        &self,
        anchor_row: i32,
        anchor_column: i32,
        cursor_row: i32,
        cursor_column: i32,
    ) -> String {
        let (rows, cols) = self.size;
        let clamp = |row: i32, col: i32| {
            (
                row.clamp(0, i32::from(rows) - 1) as usize,
                col.clamp(0, i32::from(cols) - 1) as usize,
            )
        };
        let a = clamp(anchor_row, anchor_column);
        let b = clamp(cursor_row, cursor_column);
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        let mut text = String::new();
        for row in start.0..=end.0 {
            let line = &self.rows[row];
            let from = if row == start.0 { start.1 } else { 0 };
            let to = if row == end.0 {
                end.1 + 1
            } else {
                usize::from(cols)
            };
            let mut previous_column = from;
            let mut previous_wide = false;
            for column in from..to.min(line.raw.len()) {
                if previous_wide {
                    previous_wide = false;
                    continue;
                }
                let Some(cell) = &line.raw[column] else {
                    continue;
                };
                previous_wide = cell.is_wide();
                if cell.has_contents() {
                    text.extend(std::iter::repeat_n(
                        ' ',
                        column.saturating_sub(previous_column),
                    ));
                    text.push_str(cell.contents());
                    previous_column = column + if cell.is_wide() { 2 } else { 1 };
                }
            }
            if row != end.0 && !line.wrapped {
                text.push('\n');
            }
        }
        text
    }
}

pub(super) enum Event {
    Output(Vec<u8>),
    Wake,
}

#[derive(Default)]
struct Commands {
    resize: Option<(u16, u16)>,
    bottom: bool,
    wheels: VecDeque<TerminalWheel>,
    closed: bool,
}

#[derive(Default)]
struct Latest {
    frame: Option<Arc<TerminalFrame>>,
    queued_at: Option<Instant>,
    error: Option<String>,
}

pub(super) struct TerminalOutput {
    pub sender: mpsc::SyncSender<Event>,
    commands: Arc<Mutex<Commands>>,
    latest: Arc<Mutex<Latest>>,
    signal: Arc<OutputSignal>,
    visible: Arc<AtomicBool>,
    pub frame: Arc<TerminalFrame>,
    last_stall: Option<Instant>,
}

impl TerminalOutput {
    pub fn spawn(rows: u16, columns: u16, writer: TerminalInput) -> std::io::Result<Self> {
        let parser =
            vt100::Parser::new_with_callbacks(rows, columns, 10_000, TerminalCallbacks::default());
        let frame = Arc::new(TerminalFrame::capture(parser.screen(), None));
        let (sender, receiver) = mpsc::sync_channel(64);
        let commands = Arc::new(Mutex::new(Commands::default()));
        let latest = Arc::new(Mutex::new(Latest::default()));
        let signal = Arc::new(OutputSignal::default());
        let visible = Arc::new(AtomicBool::new(true));
        let work_visible = visible.clone();
        let (work_commands, work_latest, work_signal, work_frame) = (
            commands.clone(),
            latest.clone(),
            signal.clone(),
            frame.clone(),
        );
        std::thread::Builder::new()
            .name("araseo-terminal-parser".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(
                        parser,
                        writer,
                        receiver,
                        &work_commands,
                        &work_latest,
                        &work_signal,
                        work_frame,
                        &work_visible,
                    );
                }));
                if outcome.is_err() {
                    work_latest.lock().unwrap_or_else(|e| e.into_inner()).error =
                        Some("Terminal output worker panicked".into());
                    work_signal.notify();
                }
            })?;
        Ok(Self {
            sender,
            commands,
            latest,
            signal,
            visible,
            frame,
            last_stall: None,
        })
    }

    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.signal.set_waker(Arc::new(waker));
    }

    pub fn set_visible(&self, visible: bool) {
        if self.visible.swap(visible, Ordering::Relaxed) != visible {
            let _ = self.sender.try_send(Event::Wake);
        }
    }

    pub fn poll(&mut self) -> bool {
        // Never wait for a parser or copy a screen while holding this lock.
        let Ok(mut latest) = self.latest.try_lock() else {
            return false;
        };
        self.signal.clear_pending();
        let frame = latest.frame.take();
        let queued_at = latest.queued_at.take();
        let error = latest.error.take();
        drop(latest);
        if let Some(error) = error {
            crate::diagnostics_log::event(&error);
        }
        let Some(frame) = frame else {
            return false;
        };
        let waited = queued_at.map_or(Duration::ZERO, |queued| queued.elapsed());
        if waited >= Duration::from_millis(250)
            && self
                .last_stall
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(60))
        {
            crate::diagnostics_log::event(&format!(
                "terminal UI wait_ms={} latest_frame_age_ms={}",
                waited.as_millis(),
                frame.created.elapsed().as_millis()
            ));
            self.last_stall = Some(Instant::now());
        }
        self.frame = frame;
        true
    }

    pub fn resize(&self, rows: u16, columns: u16) {
        self.commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .resize = Some((rows, columns));
        let _ = self.sender.try_send(Event::Wake);
    }

    pub fn bottom(&self) {
        let mut commands = self.commands.lock().unwrap_or_else(|e| e.into_inner());
        commands.bottom = true;
        commands.wheels.clear();
        drop(commands);
        let _ = self.sender.try_send(Event::Wake);
    }

    pub fn scroll(&self, wheel: TerminalWheel) -> bool {
        let mut commands = self.commands.lock().unwrap_or_else(|e| e.into_inner());
        if commands.wheels.len() >= MAX_WHEELS {
            return false;
        }
        commands.wheels.push_back(wheel);
        drop(commands);
        let _ = self.sender.try_send(Event::Wake);
        true
    }
}

impl Drop for TerminalOutput {
    fn drop(&mut self) {
        self.commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        let _ = self.sender.try_send(Event::Wake);
    }
}

fn run(
    mut parser: vt100::Parser<TerminalCallbacks>,
    mut writer: TerminalInput,
    receiver: mpsc::Receiver<Event>,
    commands: &Mutex<Commands>,
    latest: &Mutex<Latest>,
    signal: &OutputSignal,
    mut previous: Arc<TerminalFrame>,
    visible: &AtomicBool,
) {
    let mut dirty = false;
    let mut incoming: Option<Vec<u8>> = None;
    let mut next_frame = Instant::now();
    let mut last_log = Instant::now();
    let (mut bytes, mut frames, mut coalesced, mut snapshot_time) =
        (0u64, 0u64, 0u64, Duration::ZERO);
    loop {
        // Check commands between every bounded input chunk, even under a flood.
        let control = {
            let mut pending = commands.lock().unwrap_or_else(|e| e.into_inner());
            if pending.closed {
                return;
            }
            std::mem::take(&mut *pending)
        };
        if let Some((rows, columns)) = control.resize {
            parser.screen_mut().set_size(rows, columns);
            dirty = true;
            next_frame = Instant::now();
        }
        if control.bottom {
            dirty |= parser.screen().scrollback() > 0;
            parser.screen_mut().set_scrollback(0);
        }
        for wheel in control.wheels {
            dirty |= if let Some(input) = mouse_scroll_input(parser.screen(), wheel) {
                report_mouse_scroll(parser.screen_mut(), &mut writer, &input)
            } else {
                scroll_screen(parser.screen_mut(), wheel.rows)
            };
        }
        if let Some(chunk) = incoming.take() {
            bytes += chunk.len() as u64;
            parser.process(&chunk);
            // Protocol replies progress even while the UI is paused.
            write_terminal_replies(&mut writer, parser.callbacks_mut());
            dirty = true;
        }
        let presenting = visible.load(Ordering::Relaxed);
        if dirty && presenting && Instant::now() >= next_frame {
            let started = Instant::now();
            let frame = Arc::new(TerminalFrame::capture(parser.screen(), Some(&previous)));
            snapshot_time += started.elapsed();
            if !frame.same_view(&previous) {
                let old = {
                    let mut mailbox = latest.lock().unwrap_or_else(|e| e.into_inner());
                    if mailbox.frame.is_none() {
                        mailbox.queued_at = Some(Instant::now());
                    }
                    mailbox.frame.replace(frame.clone())
                };
                coalesced += u64::from(old.is_some());
                drop(old);
                previous = frame;
                frames += 1;
                signal.notify();
            }
            dirty = false;
            next_frame = Instant::now() + FRAME_INTERVAL;
        }
        if last_log.elapsed() >= Duration::from_secs(60) && bytes > 0 {
            crate::diagnostics_log::event(&format!(
                "terminal output bytes={bytes} frames={frames} coalesced={coalesced} snapshot_ms={}",
                snapshot_time.as_millis()
            ));
            (bytes, frames, coalesced, snapshot_time) = (0, 0, 0, Duration::ZERO);
            last_log = Instant::now();
        }
        let timeout = if dirty && presenting {
            next_frame.saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(1)
        };
        match receiver.recv_timeout(timeout) {
            Ok(Event::Output(chunk)) => incoming = Some(chunk),
            Ok(Event::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Replies(mpsc::Sender<Vec<u8>>);
    impl Write for Replies {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.send(bytes.to_vec()).unwrap();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn drains_a_flood_and_answers_split_queries_while_ui_never_polls() {
        let (replies, received) = mpsc::channel();
        let writer = TerminalInput::new(Replies(replies)).unwrap();
        let mut output = TerminalOutput::spawn(6, 40, writer).unwrap();
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed = wakes.clone();
        output.set_waker(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        });
        let sender = output.sender.clone();
        let (finished, completion) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            // Eight times the bounded pipe queue, with split VT and UTF-8.
            for _ in 0..512 {
                sender.send(Event::Output(vec![b'x'; 8192])).unwrap();
            }
            sender
                .send(Event::Output(b"\x1b[2J\x1b[H\x1b[3".to_vec()))
                .unwrap();
            sender
                .send(Event::Output(
                    [b"1m".as_slice(), &"한".as_bytes()[..1]].concat(),
                ))
                .unwrap();
            sender
                .send(Event::Output(
                    [&"한".as_bytes()[1..], b" DONE\x1b[2;6H\x1b[6"].concat(),
                ))
                .unwrap();
            sender
                .send(Event::Output(b"n\x1b]10;?\x1b".to_vec()))
                .unwrap();
            sender.send(Event::Output(b"\\".to_vec())).unwrap();
            finished.send(()).unwrap();
        });
        completion
            .recv_timeout(Duration::from_secs(10))
            .expect("output must drain without UI callbacks");
        let mut replies = received.recv_timeout(Duration::from_secs(5)).unwrap();
        while !replies.ends_with(b"\x1b\\") {
            replies.extend(received.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        assert_eq!(replies, b"\x1b[2;6R\x1b]10;rgb:ffff/ffff/ffff\x1b\\");
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "a paused UI must have only one outstanding notification"
        );
        producer.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            output.poll();
            if output.frame.selection_text(0, 0, 5, 39).contains("한 DONE") {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(output.frame.rows[0].cells[0].foreground, [0xcc, 0x66, 0x66]);
    }

    #[test]
    fn resize_and_reply_are_processed_without_ui_and_old_frames_are_replaced() {
        let (replies, received) = mpsc::channel();
        let mut output =
            TerminalOutput::spawn(24, 80, TerminalInput::new(Replies(replies)).unwrap()).unwrap();
        output.resize(12, 50);
        output.resize(8, 30);
        output
            .sender
            .send(Event::Output(b"\x1b[18t\x1b[HOLD".to_vec()))
            .unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            b"\x1b[8;8;30t"
        );
        // Wait for two distinct presentation frames without consuming either.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if output
                .latest
                .lock()
                .unwrap()
                .frame
                .as_ref()
                .is_some_and(|f| f.selection_text(0, 0, 7, 29).contains("OLD"))
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        output
            .sender
            .send(Event::Output(b"\x1b[2J\x1b[HNEW".to_vec()))
            .unwrap();
        loop {
            if output
                .latest
                .lock()
                .unwrap()
                .frame
                .as_ref()
                .is_some_and(|f| f.selection_text(0, 0, 7, 29).contains("NEW"))
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(output.poll());
        assert_eq!(output.frame.size, (8, 30));
        assert!(output.frame.selection_text(0, 0, 7, 29).contains("NEW"));
        assert!(!output.poll());
    }

    #[test]
    fn changed_rows_reuse_other_rows_and_selection_matches_the_terminal() {
        let mut parser = vt100::Parser::new(5, 12, 100);
        parser.process("abcdefghijkl한글\r\n\r\n  spaced  \x1b[?25l".as_bytes());
        let first = TerminalFrame::capture(parser.screen(), None);
        for a in 0..5 {
            for b in 0..5 {
                for col in [0, 1, 2, 11] {
                    assert_eq!(
                        first.selection_text(a, col, b, 11 - col),
                        super::super::selection_text(parser.screen(), a, col, b, 11 - col)
                    );
                }
            }
        }
        parser.process(b"\x1b[5;1Hchanged");
        let second = TerminalFrame::capture(parser.screen(), Some(&first));
        for row in 0..4 {
            assert!(Arc::ptr_eq(&first.rows[row], &second.rows[row]));
        }
        assert!(!Arc::ptr_eq(&first.rows[4], &second.rows[4]));
        // Snapshots contain the viewport only, even after history grows.
        parser.process("history\r\n".repeat(1_000).as_bytes());
        parser.screen_mut().set_scrollback(500);
        let history = TerminalFrame::capture(parser.screen(), None);
        assert_eq!(history.rows.len(), 5);
        assert_eq!(
            history.selection_text(0, 0, 4, 11),
            super::super::selection_text(parser.screen(), 0, 0, 4, 11)
        );
        parser.screen_mut().set_size(5, 20);
        let wider = TerminalFrame::capture(parser.screen(), None);
        let unchanged = TerminalFrame::capture(parser.screen(), Some(&wider));
        assert!(
            wider
                .rows
                .iter()
                .zip(&unchanged.rows)
                .all(|(a, b)| Arc::ptr_eq(a, b))
        );
    }

    #[test]
    fn a_hidden_terminal_keeps_answering_and_publishes_the_latest_view_on_activation() {
        let (replies, received) = mpsc::channel();
        let mut output =
            TerminalOutput::spawn(5, 40, TerminalInput::new(Replies(replies)).unwrap()).unwrap();
        output.set_visible(false);
        output
            .sender
            .send(Event::Output(b"hidden output\x1b[18t".to_vec()))
            .unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            b"\x1b[8;5;40t"
        );
        assert!(output.latest.lock().unwrap().frame.is_none());
        output.set_visible(true);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            output.poll();
            if output
                .frame
                .selection_text(0, 0, 4, 39)
                .contains("hidden output")
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn dropping_a_busy_terminal_stops_its_parser_and_unblocks_its_reader() {
        let output =
            TerminalOutput::spawn(6, 40, TerminalInput::new(std::io::sink()).unwrap()).unwrap();
        let commands = Arc::downgrade(&output.commands);
        let sender = output.sender.clone();
        let (finished, completion) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            while sender.send(Event::Output(vec![b'x'; 8192])).is_ok() {}
            finished.send(()).unwrap();
        });
        drop(output);
        completion.recv_timeout(Duration::from_secs(5)).unwrap();
        producer.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while commands.upgrade().is_some() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
