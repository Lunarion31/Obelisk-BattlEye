//! The speed the player may limit Steam downloads to.
//!
//! A download paces the requests of all its depots together: each reserves its
//! bytes on one timeline, so the limit holds for their total, not for each
//! connection.

use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{Stream, StreamExt};
use tokio::time::{Instant, sleep_until};

const BYTES_PER_MEGABIT: u64 = 125_000;

/// The most Steam may download, in megabits per second; zero for no limit.
static SPEED_LIMIT: AtomicU64 = AtomicU64::new(0);

/// Limits the downloads that start from now on: one under way keeps its pace.
pub fn set_speed_limit(megabits_per_second: u64) {
    SPEED_LIMIT.store(megabits_per_second, Ordering::Relaxed);
}

/// One download's pace.
pub(super) struct SpeedLimiter {
    /// `None` leaves a response as it comes.
    bytes_per_second: Option<u64>,
    /// When the bytes reserved so far have all passed.
    next: Mutex<Instant>,
}

impl SpeedLimiter {
    /// A limiter at the limit as it stands.
    pub(super) fn current() -> Self {
        Self::at(SPEED_LIMIT.load(Ordering::Relaxed))
    }

    pub(super) fn at(megabits_per_second: u64) -> Self {
        Self {
            bytes_per_second: (megabits_per_second > 0)
                .then(|| megabits_per_second.saturating_mul(BYTES_PER_MEGABIT)),
            next: Mutex::new(Instant::now()),
        }
    }

    /// `parts`, each one held back until its bytes fit the pace.
    pub(super) fn pace<B: AsRef<[u8]>, X>(
        &self,
        parts: impl Stream<Item = Result<B, X>>,
    ) -> impl Stream<Item = Result<B, X>> {
        parts.then(move |part| async move {
            if let Ok(bytes) = &part {
                self.pass(bytes.as_ref().len()).await;
            }
            part
        })
    }

    /// Returns once `bytes` more may pass. A pause is no credit: after one,
    /// the bytes do not pass at once.
    async fn pass(&self, bytes: usize) {
        let Some(rate) = self.bytes_per_second else {
            return;
        };
        let passed = {
            let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
            *next =
                (*next).max(Instant::now()) + Duration::from_secs_f64(bytes as f64 / rate as f64);
            *next
        };
        sleep_until(passed).await;
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream;

    use super::*;

    /// 12.5 kB: a tenth of a second at one megabit per second.
    const TENTH: usize = 12_500;

    #[tokio::test]
    async fn concurrent_requests_share_one_pace() {
        let limiter = SpeedLimiter::at(1);
        let start = Instant::now();
        tokio::join!(
            limiter.pass(TENTH),
            limiter.pass(TENTH),
            limiter.pass(TENTH)
        );
        assert!(start.elapsed() >= Duration::from_millis(300));
    }

    #[tokio::test]
    async fn no_limit_neither_waits_nor_reserves() {
        let limiter = SpeedLimiter::at(0);
        let reserved = *limiter.next.lock().unwrap();
        tokio::time::timeout(Duration::from_millis(100), limiter.pass(usize::MAX))
            .await
            .unwrap();
        assert_eq!(*limiter.next.lock().unwrap(), reserved);
    }

    #[tokio::test]
    async fn a_pause_does_not_let_the_next_bytes_through_at_once() {
        let limiter = SpeedLimiter::at(1);
        *limiter.next.lock().unwrap() = Instant::now() - Duration::from_secs(10);
        let start = Instant::now();
        limiter.pass(TENTH).await;
        limiter.pass(TENTH).await;
        assert!(start.elapsed() >= Duration::from_millis(200));
    }

    #[tokio::test]
    async fn a_paced_stream_holds_each_part_and_passes_a_failure_on() {
        let limiter = SpeedLimiter::at(1);
        let parts = stream::iter([Ok(vec![0; TENTH]), Ok(vec![1; TENTH]), Err("lost")]);
        let start = Instant::now();
        let paced: Vec<_> = limiter.pace(parts).collect().await;
        assert!(start.elapsed() >= Duration::from_millis(200));
        assert_eq!(paced, [Ok(vec![0; TENTH]), Ok(vec![1; TENTH]), Err("lost")]);
    }
}
