//! Lazy workers with one pending job and one result. Failures remain observable.
use std::sync::{Arc, Condvar, Mutex};
type Job<T> = Box<dyn FnOnce() -> T + Send>;
struct Mailbox<T> {
    job: Option<(u64, Job<T>)>,
    result: Option<(u64, Result<T, String>)>,
    generation: u64,
    closed: bool,
}
pub struct Background<T> {
    mailbox: Arc<(Mutex<Mailbox<T>>, Condvar)>,
    started: bool,
    error: Option<String>,
}
impl<T> Default for Background<T> {
    fn default() -> Self {
        Self {
            mailbox: Arc::new((
                Mutex::new(Mailbox {
                    job: None,
                    result: None,
                    generation: 0,
                    closed: false,
                }),
                Condvar::new(),
            )),
            started: false,
            error: None,
        }
    }
}
impl<T: Send + 'static> Background<T> {
    pub fn request(&mut self, job: impl FnOnce() -> T + Send + 'static) {
        self.cancel();
        if !self.started {
            let worker = self.mailbox.clone();
            match std::thread::Builder::new()
                .name("araseo-worker".into())
                .spawn(move || {
                    loop {
                        let job = {
                            let (lock, ready) = &*worker;
                            let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                            while state.job.is_none() && !state.closed {
                                state = ready.wait(state).unwrap_or_else(|e| e.into_inner());
                            }
                            if state.closed {
                                return;
                            }
                            state.job.take().unwrap()
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.1))
                            .map_err(|panic| {
                                panic
                                    .downcast_ref::<&str>()
                                    .map(|s| (*s).to_owned())
                                    .or_else(|| panic.downcast_ref::<String>().cloned())
                                    .unwrap_or_else(|| "worker panicked".into())
                            });
                        let mut state = worker.0.lock().unwrap_or_else(|e| e.into_inner());
                        if state.closed {
                            return;
                        }
                        if job.0 == state.generation {
                            state.result = Some((job.0, result));
                        }
                    }
                }) {
                Ok(_) => self.started = true,
                Err(error) => {
                    self.error = Some(format!("Could not start background worker: {error}"));
                    return;
                }
            }
        }
        let mut state = self.mailbox.0.lock().unwrap_or_else(|e| e.into_inner());
        let generation = state.generation;
        state.job = Some((generation, Box::new(job)));
        self.mailbox.1.notify_one();
    }
}
impl<T> Background<T> {
    pub fn cancel(&mut self) {
        let discarded = {
            let mut state = self.mailbox.0.lock().unwrap_or_else(|e| e.into_inner());
            state.generation = state.generation.wrapping_add(1);
            (state.job.take(), state.result.take())
        };
        drop(discarded);
        self.error = None;
    }
    pub fn poll(&mut self) -> Option<T> {
        let result = self
            .mailbox
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .result
            .take();
        match result.map(|(_, result)| result) {
            Some(Ok(value)) => Some(value),
            Some(Err(error)) => {
                self.error = Some(format!("Background task failed: {error}"));
                None
            }
            None => None,
        }
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}
impl<T> Drop for Background<T> {
    fn drop(&mut self) {
        let mut state = self.mailbox.0.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.job = None;
        self.mailbox.1.notify_one();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    fn wait<T>(worker: &mut Background<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = worker.poll() {
                return value;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn slow_work_does_not_block_requests_and_obsolete_work_is_discarded() {
        let mut worker = Background::default();
        assert!(!worker.started);
        let (started, ready) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        worker.request(move || {
            started.send(()).unwrap();
            blocked.recv().unwrap();
            1
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.request(|| panic!("superseded job"));
        worker.request(|| 3);
        release.send(()).unwrap();
        assert_eq!(wait(&mut worker), 3);
    }
    #[test]
    fn cancellation_rejects_an_already_completed_result() {
        let mut worker = Background::default();
        worker.request(|| 1);
        assert_eq!(wait(&mut worker), 1);
        worker.cancel();
        assert!(worker.poll().is_none());
    }
    #[test]
    fn task_panic_is_reported_and_worker_accepts_next_request() {
        let mut worker = Background::<u32>::default();
        worker.request(|| panic!("broken task"));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            worker.poll();
            if let Some(error) = worker.take_error() {
                assert!(error.contains("broken task"));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        worker.request(|| 9);
        assert_eq!(wait(&mut worker), 9);
    }
}
