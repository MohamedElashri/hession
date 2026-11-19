//! Bounded debounce scheduling shared by native observation and recovery tests.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
pub const MAX_PENDING: usize = 512;
const DEBOUNCE: Duration = Duration::from_millis(400);
const MAX_DELAY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Pending {
    paths: BTreeMap<PathBuf, (Instant, Instant)>,
    reconcile: bool,
}
impl Pending {
    pub fn push(&mut self, path: &Path, now: Instant) {
        if let Some((_, last)) = self.paths.get_mut(path) {
            *last = now;
            return;
        }
        if self.paths.len() >= MAX_PENDING {
            self.reconcile = true;
            return;
        }
        self.paths.insert(path.into(), (now, now));
    }
    pub fn overflow(&mut self) {
        self.reconcile = true;
    }
    pub fn len(&self) -> usize {
        self.paths.len()
    }
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && !self.reconcile
    }
    /// Sleep until work is due; a quiet native observer needs no debounce tick.
    pub fn next_ready_in(&self, now: Instant) -> Option<Duration> {
        if self.reconcile {
            return Some(Duration::ZERO);
        }
        self.paths
            .values()
            .map(|(first, last)| {
                (*last + DEBOUNCE)
                    .min(*first + MAX_DELAY)
                    .saturating_duration_since(now)
            })
            .min()
    }
    pub fn take_ready(&mut self, now: Instant) -> (Vec<PathBuf>, bool) {
        let mut ready = Vec::new();
        self.paths.retain(|path, (first, last)| {
            if now.saturating_duration_since(*last) >= DEBOUNCE
                || now.saturating_duration_since(*first) >= MAX_DELAY
            {
                ready.push(path.clone());
                false
            } else {
                true
            }
        });
        (ready, std::mem::take(&mut self.reconcile))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bursts_are_deduplicated_bounded_and_continuous_writes_cannot_starve() {
        let start = Instant::now();
        let mut queue = Pending::default();
        assert_eq!(queue.next_ready_in(start), None);
        let path = Path::new("source");
        for tick in 0..10 {
            queue.push(path, start + Duration::from_millis(tick * 100));
        }
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.next_ready_in(start + Duration::from_millis(900)),
            Some(Duration::from_millis(100))
        );
        assert!(queue
            .take_ready(start + Duration::from_millis(999))
            .0
            .is_empty());
        assert_eq!(
            queue.take_ready(start + Duration::from_secs(1)).0,
            vec![path]
        );
        for n in 0..MAX_PENDING + 1 {
            queue.push(Path::new(&n.to_string()), start);
        }
        assert_eq!(queue.len(), MAX_PENDING);
        let (paths, rescan) = queue.take_ready(start + DEBOUNCE);
        assert_eq!(paths.len(), MAX_PENDING);
        assert!(rescan);
        assert!(queue.is_empty());
        queue.overflow();
        assert_eq!(queue.next_ready_in(start), Some(Duration::ZERO));
    }
}
