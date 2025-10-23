//! Per-IP rate limiting for the finger server.
use std::net::IpAddr;
use std::sync::{Arc, Once};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use dashmap::DashMap;
use futures::future::ready;
use mockable::{Clock, DefaultClock};
use thiserror::Error;

#[derive(Debug, Clone)]
/// Tunable rate limiting parameters.
pub struct RateLimitSettings {
    /// Maximum number of requests permitted during a single window.
    pub max_requests: u32,
    /// Duration of the rolling window applied per client.
    pub window: Duration,
    /// Maximum number of distinct client entries retained simultaneously.
    pub max_entries: usize,
}

#[derive(Debug, Error)]
/// Errors emitted when a client exceeds configured limits.
pub enum RateLimitError {
    #[error("too many requests; retry in {retry_in:?}")]
    LimitExceeded { retry_in: Duration },
}

#[derive(Debug)]
struct RateEntry {
    window_start: DateTime<Utc>,
    count: u32,
}

const ENTRY_GAUGE_NAME: &str = "fingermouse.rate_limiter.entries";
static DESCRIBE_GAUGE: Once = Once::new();

/// Asynchronous rate limiter with per-IP accounting.
pub struct RateLimiter {
    clock: Arc<dyn Clock>,
    settings: RateLimitSettings,
    window: ChronoDuration,
    entries: DashMap<IpAddr, RateEntry>,
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl RateLimiter {
    /// Construct a limiter that uses the system clock for timing decisions.
    pub fn new(settings: RateLimitSettings) -> Self {
        Self::with_clock(settings, Arc::new(DefaultClock))
    }

    /// Construct a limiter using an injected clock, enabling deterministic tests.
    pub fn with_clock(mut settings: RateLimitSettings, clock: Arc<dyn Clock>) -> Self {
        DESCRIBE_GAUGE.call_once(|| {
            metrics::describe_gauge!(
                ENTRY_GAUGE_NAME,
                "Current number of active rate limiter entries."
            );
        });
        if settings.max_entries == 0 {
            settings.max_entries = 1;
        }
        let window = chrono_duration(settings.window);
        metrics::gauge!(ENTRY_GAUGE_NAME).set(0.0);
        Self {
            clock,
            settings,
            window,
            entries: DashMap::new(),
        }
    }

    /// Record an access for `ip`, enforcing the configured rate limits.
    pub async fn check(&self, ip: IpAddr) -> Result<(), RateLimitError> {
        ready(self.check_inner(ip)).await
    }

    fn check_inner(&self, ip: IpAddr) -> Result<(), RateLimitError> {
        let now = self.clock.utc();
        self.prune_stale(now);
        let outcome = {
            let mut entry = self.entries.entry(ip).or_insert_with(|| RateEntry {
                window_start: now,
                count: 0,
            });

            if now - entry.window_start >= self.window {
                entry.window_start = now;
                entry.count = 0;
            }

            if entry.count >= self.settings.max_requests {
                let elapsed = now - entry.window_start;
                let remaining = self.window - elapsed;
                let retry_in = remaining
                    .to_std()
                    .unwrap_or_else(|_| Duration::from_secs(1));
                Err(RateLimitError::LimitExceeded { retry_in })
            } else {
                entry.count += 1;
                Ok(())
            }
        };
        self.enforce_capacity(ip);
        self.record_entry_count();
        outcome
    }

    fn prune_stale(&self, now: DateTime<Utc>) {
        let cutoff = now - self.window;
        self.entries.retain(|_, entry| entry.window_start >= cutoff);
    }

    fn enforce_capacity(&self, protected: IpAddr) {
        let max_entries = self.settings.max_entries;
        if self.entries.len() <= max_entries {
            return;
        }

        while self.entries.len() > max_entries {
            let candidate = self
                .oldest_key_excluding(Some(protected))
                .or_else(|| self.oldest_key_excluding(None));
            let Some(key) = candidate else { break };
            if key == protected {
                // Skip eviction of the entry we just refreshed to avoid race-driven drops.
                break;
            }
            self.entries.remove(&key);
        }
    }

    fn oldest_key_excluding(&self, excluded: Option<IpAddr>) -> Option<IpAddr> {
        let mut selected: Option<(IpAddr, DateTime<Utc>)> = None;
        for entry in &self.entries {
            let key = *entry.key();
            if excluded.is_some_and(|target| target == key) {
                continue;
            }
            let start = entry.value().window_start;
            let should_replace = match &selected {
                Some((_, current_start)) => start < *current_start,
                None => true,
            };
            if should_replace {
                selected = Some((key, start));
            }
        }
        selected.map(|(key, _)| key)
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "Gauge API accepts f64; entry cap keeps counts within safe precision bounds."
    )]
    fn record_entry_count(&self) {
        let len = self.entries.len();
        metrics::gauge!(ENTRY_GAUGE_NAME).set(len as f64);
    }
}

fn chrono_duration(window: Duration) -> ChronoDuration {
    ChronoDuration::from_std(window).unwrap_or_else(|_| ChronoDuration::seconds(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    struct ManualClock {
        ticks: StdMutex<Vec<DateTime<Utc>>>,
    }

    impl ManualClock {
        fn from_times(times: Vec<DateTime<Utc>>) -> Arc<dyn Clock> {
            Arc::new(Self {
                ticks: StdMutex::new(times),
            })
        }
    }

    impl Clock for ManualClock {
        fn local(&self) -> DateTime<chrono::Local> {
            chrono::Local::now()
        }

        fn utc(&self) -> DateTime<Utc> {
            let mut guard = match self.ticks.lock() {
                Ok(value) => value,
                Err(poisoned) => poisoned.into_inner(),
            };
            if guard.len() > 1 {
                guard.remove(0)
            } else {
                guard.first().copied().unwrap_or_else(Utc::now)
            }
        }
    }

    fn timestamps(offsets: &[i64]) -> Vec<DateTime<Utc>> {
        let base = Utc::now();
        offsets
            .iter()
            .map(|secs| base + ChronoDuration::seconds(*secs))
            .collect()
    }

    fn setup_limiter_and_ip(
        max_requests: u32,
        window_secs: u64,
        time_offsets: &[i64],
        ip_str: &str,
    ) -> Result<(RateLimiter, IpAddr), RateLimitError> {
        let settings = RateLimitSettings {
            max_requests,
            window: Duration::from_secs(window_secs),
            max_entries: 16,
        };
        let limiter =
            RateLimiter::with_clock(settings, ManualClock::from_times(timestamps(time_offsets)));
        let ip: IpAddr = ip_str.parse().map_err(|_| RateLimitError::LimitExceeded {
            retry_in: Duration::ZERO,
        })?;
        Ok((limiter, ip))
    }

    #[tokio::test]
    async fn allows_requests_up_to_limit() -> Result<(), RateLimitError> {
        let (limiter, ip) = setup_limiter_and_ip(2, 60, &[0, 0, 10], "192.0.2.1")?;
        limiter.check(ip).await?;
        limiter.check(ip).await?;
        Ok(())
    }

    #[tokio::test]
    async fn enforces_limit() {
        let settings = RateLimitSettings {
            max_requests: 1,
            window: Duration::from_secs(60),
            max_entries: 8,
        };
        let limiter =
            RateLimiter::with_clock(settings, ManualClock::from_times(timestamps(&[0, 5])));
        let ip: IpAddr = match "198.51.100.2".parse() {
            Ok(value) => value,
            Err(err) => panic!("ip parse failed: {err}"),
        };
        assert!(limiter.check(ip).await.is_ok());
        let outcome = limiter.check(ip).await;
        assert!(matches!(
            outcome,
            Err(RateLimitError::LimitExceeded { retry_in }) if retry_in <= Duration::from_secs(60)
        ));
    }

    #[tokio::test]
    async fn resets_after_window() -> Result<(), RateLimitError> {
        let (limiter, ip) = setup_limiter_and_ip(1, 10, &[0, 15], "203.0.113.5")?;
        limiter.check(ip).await?;
        limiter.check(ip).await?;
        Ok(())
    }

    #[tokio::test]
    async fn evicts_stale_entries() {
        let times = timestamps(&[0, 0, 120]);
        let limiter = RateLimiter::with_clock(
            RateLimitSettings {
                max_requests: 5,
                window: Duration::from_secs(60),
                max_entries: 16,
            },
            ManualClock::from_times(times),
        );
        let first_ip = IpAddr::from([198, 18, 0, 1]);
        let second_ip = IpAddr::from([198, 18, 0, 2]);
        assert!(limiter.check(first_ip).await.is_ok());
        assert!(limiter.check(second_ip).await.is_ok());
        assert!(limiter.entries.contains_key(&first_ip));
        assert!(limiter.check(first_ip).await.is_ok());
        assert!(
            !limiter.entries.contains_key(&second_ip),
            "stale entry should be evicted after window lapse"
        );
    }

    #[tokio::test]
    async fn enforces_capacity_limit() {
        let offsets = timestamps(&[0, 1, 2, 3]);
        let limiter = RateLimiter::with_clock(
            RateLimitSettings {
                max_requests: 5,
                window: Duration::from_secs(300),
                max_entries: 2,
            },
            ManualClock::from_times(offsets),
        );
        let ip1 = IpAddr::from([203, 0, 113, 1]);
        let ip2 = IpAddr::from([203, 0, 113, 2]);
        let ip3 = IpAddr::from([203, 0, 113, 3]);
        assert!(limiter.check(ip1).await.is_ok());
        assert!(limiter.check(ip2).await.is_ok());
        assert_eq!(limiter.entries.len(), 2);
        assert!(limiter.check(ip3).await.is_ok());
        assert_eq!(limiter.entries.len(), 2);
        assert!(
            !limiter.entries.contains_key(&ip1),
            "oldest entry should be discarded when capacity exceeded"
        );
    }
}
