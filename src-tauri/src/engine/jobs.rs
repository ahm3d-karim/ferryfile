//! Cancellable jobs. Each conversion the UI starts runs under a job id; the
//! UI can cancel it by id. Engines don't take a token parameter: the running
//! thread carries its job's flag, and long-running code polls
//! [`is_cancelled`] (ffmpeg kills its child process; chains stop between
//! steps).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Error text a cancelled job returns; the UI shows it as "Cancelled".
pub const CANCELLED: &str = "Cancelled";

fn registry() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Marks the current thread as running job `id` until the guard drops.
pub struct JobGuard(Option<String>);

impl Drop for JobGuard {
    fn drop(&mut self) {
        CURRENT.with(|c| c.borrow_mut().take());
        if let Some(id) = &self.0 {
            registry().lock().unwrap().remove(id);
        }
    }
}

/// Starts job `id` on this thread (`None`: an uncancellable job).
pub fn begin(id: Option<&str>) -> JobGuard {
    let flag = Arc::new(AtomicBool::new(false));
    if let Some(id) = id {
        registry().lock().unwrap().insert(id.to_string(), flag.clone());
    }
    CURRENT.with(|c| *c.borrow_mut() = Some(flag));
    JobGuard(id.map(str::to_string))
}

/// Asks job `id` to stop. Returns false if no such job is running.
pub fn cancel(id: &str) -> bool {
    match registry().lock().unwrap().get(id) {
        Some(flag) => {
            flag.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

/// True if the job running on this thread was cancelled.
pub fn is_cancelled() -> bool {
    CURRENT.with(|c| c.borrow().as_ref().is_some_and(|f| f.load(Ordering::SeqCst)))
}

/// `Err(CANCELLED)` if the current job was cancelled.
pub fn check() -> Result<(), String> {
    if is_cancelled() {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_reaches_only_its_job() {
        let handle = std::thread::spawn(|| {
            let _job = begin(Some("job-a"));
            while !is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            check()
        });
        let _other = begin(Some("job-b"));
        while !registry().lock().unwrap().contains_key("job-a") {
            std::thread::yield_now();
        }
        assert!(cancel("job-a"));
        assert_eq!(handle.join().unwrap(), Err(CANCELLED.to_string()));
        assert!(!is_cancelled(), "job-b is unaffected");
        assert!(!cancel("job-a"), "finished jobs are gone");
    }
}
