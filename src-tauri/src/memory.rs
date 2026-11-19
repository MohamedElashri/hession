//! Return unused import/read buffers after work settles, with no idle polling.

pub(crate) const QUIET: std::time::Duration = std::time::Duration::from_secs(3);
pub(crate) const MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn release() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim takes no pointers and is MT-Safe. Only free pages in
    // libc-owned arenas are released; live Rust/SQLite/GTK allocations remain.
    // https://man7.org/linux/man-pages/man3/malloc_trim.3.html
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Coalesce completed imports and transcript reads. The maximum delay prevents
/// continuous work from retaining old peak allocations indefinitely. One bounded
/// channel handles a burst; its worker blocks indefinitely when idle.
pub(crate) fn schedule() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use std::sync::{mpsc, OnceLock};
        static SENDER: OnceLock<Option<mpsc::SyncSender<()>>> = OnceLock::new();
        let sender = SENDER.get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel(1);
            match std::thread::Builder::new()
                .name("idle-memory".into())
                .spawn(move || reclaim_worker(receiver, QUIET, MAX_DELAY, release))
            {
                Ok(_) => Some(sender),
                Err(error) => {
                    eprintln!("Could not start idle memory reclamation: {error}");
                    None
                }
            }
        });
        if let Some(sender) = sender {
            let _ = sender.try_send(());
        }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn reclaim_worker(
    receiver: std::sync::mpsc::Receiver<()>,
    quiet: std::time::Duration,
    maximum: std::time::Duration,
    mut reclaim: impl FnMut(),
) {
    while receiver.recv().is_ok() {
        let deadline = std::time::Instant::now() + maximum;
        loop {
            let wait = quiet.min(deadline.saturating_duration_since(std::time::Instant::now()));
            match receiver.recv_timeout(wait) {
                Ok(()) if std::time::Instant::now() < deadline => {}
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        reclaim();
    }
}

#[cfg(all(test, target_os = "linux", target_env = "gnu"))]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn continuous_reads_cannot_starve_reclamation_and_idle_has_no_repeated_callbacks() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (done, notices) = mpsc::channel();
        let worker = thread::spawn(move || {
            reclaim_worker(
                receiver,
                Duration::from_millis(30),
                Duration::from_millis(100),
                || {
                    let _ = done.send(());
                },
            )
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let _ = sender.try_send(());
            if notices.try_recv().is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "continuous reads starved reclamation"
            );
            thread::sleep(Duration::from_millis(5));
        }
        // Drain the last queued burst, then verify that a quiet worker sleeps.
        thread::sleep(Duration::from_millis(200));
        while notices.try_recv().is_ok() {}
        assert!(notices.recv_timeout(Duration::from_millis(100)).is_err());
        drop(sender);
        worker.join().unwrap();
    }
}
