//! Request pacing for downloads: a cap on parallel tasks, a random pause between task
//! starts and an exponential backoff once YouTube starts throttling (429 / "not a bot").
//!
//! All waiting goes through `tokio::time`, so tests run instantly under `time::pause()`.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacerConfig {
    pub max_parallel: usize,
    /// Random pause between consecutive task starts, inclusive range.
    pub pause_min: Duration,
    pub pause_max: Duration,
    /// First backoff after a rate limit; doubles with each further one.
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
}

impl Default for PacerConfig {
    fn default() -> Self {
        Self {
            max_parallel: 2,
            pause_min: Duration::from_secs(1),
            pause_max: Duration::from_secs(3),
            backoff_base: Duration::from_secs(30),
            backoff_cap: Duration::from_secs(15 * 60),
        }
    }
}

/// How a paced task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// HTTP 429 or "confirm you're not a bot".
    RateLimited,
    /// Any other failure; does not change the pacing.
    OtherError,
}

/// Held while a task runs; dropping it frees the slot.
#[derive(Debug)]
pub struct Permit {
    _slot: OwnedSemaphorePermit,
}

#[derive(Debug)]
struct State {
    /// Earliest start of the next task.
    next_start: Instant,
    /// No task starts before this while throttled.
    backoff_until: Option<Instant>,
    /// Rate limits in a row; the next backoff is `base * 2^(strikes - 1)`.
    strikes: u32,
}

#[derive(Debug)]
pub struct Pacer {
    config: PacerConfig,
    slots: Arc<Semaphore>,
    state: Mutex<State>,
}

impl Pacer {
    /// Default pauses (1–3 s) and backoff (30 s doubling up to 15 min).
    pub fn new(max_parallel: usize) -> Self {
        Self::with_config(PacerConfig {
            max_parallel,
            ..PacerConfig::default()
        })
    }

    pub fn with_config(config: PacerConfig) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(config.max_parallel.max(1))),
            state: Mutex::new(State {
                next_start: Instant::now(),
                backoff_until: None,
                strikes: 0,
            }),
            config,
        }
    }

    /// Waits for a free slot, the pause after the previous start and any active backoff.
    pub async fn acquire(&self) -> Permit {
        let slot = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        loop {
            let target = {
                let mut st = self.lock();
                let now = Instant::now();
                let target = st
                    .backoff_until
                    .map_or(st.next_start, |b| b.max(st.next_start));
                if target <= now {
                    st.next_start = now + self.random_pause();
                    break;
                }
                target
            };
            // Re-check after sleeping: a backoff may have started meanwhile.
            tokio::time::sleep_until(target).await;
        }
        Permit { _slot: slot }
    }

    /// Feeds the result of a task back into the pacing.
    pub fn report(&self, outcome: Outcome) {
        let mut st = self.lock();
        let now = Instant::now();
        match outcome {
            Outcome::RateLimited => {
                st.strikes = st.strikes.saturating_add(1);
                let delay = self.backoff_for(st.strikes);
                let until = now + delay;
                st.backoff_until = Some(st.backoff_until.map_or(until, |b| b.max(until)));
                tracing::info!(secs = delay.as_secs(), "YouTube ограничил запросы, пауза");
            }
            Outcome::Ok => {
                // A task that was already running when the backoff began does not reset it.
                if st.backoff_until.is_none_or(|b| b <= now) {
                    st.strikes = 0;
                    st.backoff_until = None;
                }
            }
            Outcome::OtherError => {}
        }
    }

    /// Time left in the current backoff, for the UI.
    pub fn backoff_remaining(&self) -> Option<Duration> {
        let st = self.lock();
        let now = Instant::now();
        st.backoff_until.filter(|b| *b > now).map(|b| b - now)
    }

    fn backoff_for(&self, strikes: u32) -> Duration {
        let factor = 2u32.saturating_pow(strikes.saturating_sub(1).min(20));
        self.config
            .backoff_base
            .saturating_mul(factor)
            .min(self.config.backoff_cap)
    }

    fn random_pause(&self) -> Duration {
        let min = self.config.pause_min.as_millis() as u64;
        let max = (self.config.pause_max.as_millis() as u64).max(min);
        Duration::from_millis(fastrand::u64(min..=max))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn config(pause: Duration) -> PacerConfig {
        PacerConfig {
            pause_min: pause,
            pause_max: pause,
            ..PacerConfig::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_doubles_up_to_cap_and_resets_after_success() {
        let p = Pacer::with_config(config(Duration::ZERO));
        let mut seen = Vec::new();
        for _ in 0..7 {
            p.report(Outcome::RateLimited);
            seen.push(p.backoff_remaining().unwrap().as_secs());
            tokio::time::advance(Duration::from_secs(1000)).await;
        }
        assert_eq!(seen, [30, 60, 120, 240, 480, 900, 900]);

        p.report(Outcome::Ok);
        p.report(Outcome::RateLimited);
        assert_eq!(p.backoff_remaining(), Some(Duration::from_secs(30)));
    }

    #[tokio::test(start_paused = true)]
    async fn success_during_backoff_does_not_reset_it() {
        let p = Pacer::with_config(config(Duration::ZERO));
        p.report(Outcome::RateLimited);
        p.report(Outcome::Ok);
        p.report(Outcome::OtherError);
        assert_eq!(p.backoff_remaining(), Some(Duration::from_secs(30)));
        p.report(Outcome::RateLimited);
        assert_eq!(p.backoff_remaining(), Some(Duration::from_secs(60)));
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_waits_for_backoff() {
        let p = Pacer::with_config(config(Duration::ZERO));
        p.report(Outcome::RateLimited);
        let t0 = Instant::now();
        drop(p.acquire().await);
        assert_eq!(t0.elapsed(), Duration::from_secs(30));
        assert_eq!(p.backoff_remaining(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn pauses_between_consecutive_starts() {
        let p = Pacer::new(2);
        let t0 = Instant::now();
        drop(p.acquire().await);
        assert_eq!(
            t0.elapsed(),
            Duration::ZERO,
            "the first task starts at once"
        );
        drop(p.acquire().await);
        let gap = t0.elapsed();
        assert!(
            gap >= Duration::from_secs(1) && gap <= Duration::from_secs(3),
            "{gap:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn caps_concurrency() {
        let p = Arc::new(Pacer::with_config(config(Duration::from_millis(100))));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..6)
            .map(|_| {
                let (p, running, peak) = (p.clone(), running.clone(), peak.clone());
                tokio::spawn(async move {
                    let _permit = p.acquire().await;
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    p.report(Outcome::Ok);
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }
}
