//! A bounded ordered input queue. Blocking pipe writes never hold its mutex.
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex};
const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;
#[derive(Default)]
struct Queue {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
    closed: bool,
    error: Option<String>,
}
#[derive(Clone)]
pub struct TerminalInput {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    // The writer thread holds only the queue. The last sender closes it.
    _lifetime: Arc<InputLifetime>,
}
struct InputLifetime(Arc<(Mutex<Queue>, Condvar)>);
impl TerminalInput {
    pub fn new(mut writer: impl Write + Send + 'static) -> io::Result<Self> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let worker = queue.clone();
        std::thread::Builder::new()
            .name("araseo-input".into())
            .spawn(move || {
                loop {
                    let chunk = {
                        let mut state = worker.0.lock().unwrap_or_else(|e| e.into_inner());
                        while state.chunks.is_empty() && !state.closed {
                            state = worker.1.wait(state).unwrap_or_else(|e| e.into_inner());
                        }
                        if state.closed {
                            return;
                        }
                        state.chunks.pop_front().unwrap()
                    };
                    let result = writer.write_all(&chunk).and_then(|_| writer.flush());
                    let mut state = worker.0.lock().unwrap_or_else(|e| e.into_inner());
                    state.bytes = state.bytes.saturating_sub(chunk.len());
                    if let Err(error) = result {
                        state.error = Some(format!("Terminal input failed: {error}"));
                        state.closed = true;
                        state.chunks.clear();
                        return;
                    }
                }
            })?;
        Ok(Self {
            _lifetime: Arc::new(InputLifetime(queue.clone())),
            queue,
        })
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.queue
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .take()
    }
}
impl Write for TerminalInput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.queue.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal input is closed",
            ));
        }
        if bytes.len() > MAX_PENDING_BYTES.saturating_sub(state.bytes) {
            state.error = Some("Terminal input queue is full; input was not sent".into());
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal input queue is full",
            ));
        }
        if !bytes.is_empty() {
            state.bytes += bytes.len();
            state.chunks.push_back(bytes.to_vec());
            self.queue.1.notify_one();
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for InputLifetime {
    fn drop(&mut self) {
        let mut state = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.chunks.clear();
        self.0.1.notify_one();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;
    struct Recorded(mpsc::Sender<Vec<u8>>);
    impl Write for Recorded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.send(bytes.to_vec()).unwrap();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn dropping_one_sender_does_not_close_shared_protocol_input() {
        let (sent, received) = mpsc::channel();
        let input = TerminalInput::new(Recorded(sent)).unwrap();
        let mut protocol = input.clone();
        drop(input);
        protocol.write_all(b"\x1b[2;6R").unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            b"\x1b[2;6R"
        );
        drop(protocol);
    }
    struct Blocked {
        started: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        written: mpsc::Sender<Vec<u8>>,
    }
    impl Write for Blocked {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.started.send(()).unwrap();
            self.release.recv().unwrap();
            self.written.send(bytes.to_vec()).unwrap();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn blocked_writer_does_not_block_enqueue_and_order_is_preserved() {
        let (started, ready) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (written, output) = mpsc::channel();
        let mut input = TerminalInput::new(Blocked {
            started,
            release: blocked,
            written,
        })
        .unwrap();
        input.write_all(b"first").unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        input.write_all(b"second").unwrap();
        assert!(input.write_all(&vec![0; MAX_PENDING_BYTES]).is_err());
        assert!(input.take_error().unwrap().contains("full"));
        release.send(()).unwrap();
        assert_eq!(
            output.recv_timeout(Duration::from_secs(5)).unwrap(),
            b"first"
        );
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        release.send(()).unwrap();
        assert_eq!(
            output.recv_timeout(Duration::from_secs(5)).unwrap(),
            b"second"
        );
    }
}
