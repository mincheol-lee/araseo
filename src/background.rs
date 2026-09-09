//! A single worker with a latest-request mailbox. Slow I/O never blocks the UI,
//! queued obsolete work is replaced, and obsolete results cannot be applied.
use std::sync::{Arc, Condvar, Mutex, mpsc};

type Job<T> = Box<dyn FnOnce() -> T + Send>;
struct Mailbox<T> {
    job: Option<(u64, Job<T>)>,
    closed: bool,
}

pub struct Background<T> {
    mailbox: Arc<(Mutex<Mailbox<T>>, Condvar)>,
    results: mpsc::Receiver<(u64, T)>,
    generation: u64,
}

impl<T: Send + 'static> Default for Background<T> {
    fn default() -> Self {
        let mailbox = Arc::new((
            Mutex::new(Mailbox::<T> {
                job: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let worker = mailbox.clone();
        let (sender, results) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let job = {
                    let (lock, ready) = &*worker;
                    let mut state = lock.lock().unwrap();
                    while state.job.is_none() && !state.closed {
                        state = ready.wait(state).unwrap();
                    }
                    if state.closed {
                        return;
                    }
                    state.job.take().unwrap()
                };
                if sender.send((job.0, job.1())).is_err() {
                    return;
                }
            }
        });
        Self {
            mailbox,
            results,
            generation: 0,
        }
    }
}

impl<T> Background<T> {
    pub fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.mailbox.0.lock().unwrap().job = None;
    }

    pub fn request(&mut self, job: impl FnOnce() -> T + Send + 'static) {
        self.cancel();
        self.mailbox.0.lock().unwrap().job = Some((self.generation, Box::new(job)));
        self.mailbox.1.notify_one();
    }

    pub fn poll(&mut self) -> Option<T> {
        let mut latest = None;
        while let Ok((generation, result)) = self.results.try_recv() {
            if generation == self.generation {
                latest = Some(result);
            }
        }
        latest
    }
}

impl<T> Drop for Background<T> {
    fn drop(&mut self) {
        let mut state = self.mailbox.0.lock().unwrap();
        state.closed = true;
        state.job = None;
        self.mailbox.1.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn slow_work_does_not_block_requests_and_obsolete_work_is_discarded() {
        let mut worker = Background::default();
        let (started, wait_started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        worker.request(move || {
            started.send(()).unwrap();
            blocked.recv().unwrap();
            1
        });
        wait_started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(worker.poll(), None);
        worker.request(|| panic!("superseded queued job must never run"));
        worker.request(|| 3);
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = worker.poll() {
                assert_eq!(value, 3);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn cancellation_rejects_an_already_completed_result() {
        let mut worker = Background::default();
        let (done, wait_done) = mpsc::channel();
        worker.request(move || {
            done.send(()).unwrap();
            42
        });
        wait_done.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.cancel();
        worker.request(|| 7);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = worker.poll() {
                assert_eq!(value, 7);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
