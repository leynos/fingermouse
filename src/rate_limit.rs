//! Per-IP rate limiting for the finger server.
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use mockable::{Clock, DefaultClock};
use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct RateLimitSettings {
    pub max_requests: u32,
    pub window: Duration,
}

#[derive(Debug, Error)]
pub enum RateLimitError {
    #[error("too many requests; retry in {retry_in:?}")]
    LimitExceeded { retry_in: Duration },
}

#[derive(Debug)]
struct RateEntry {
    window_start: DateTime<Utc>,
    count: u32,
}

pub struct RateLimiter {
    clock: Arc<dyn Clock>,
    settings: RateLimitSettings,
    window: ChronoDuration,
    entries: Mutex<HashMap<IpAddr, RateEntry>>,
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl RateLimiter {
    pub fn new(settings: RateLimitSettings) -> Self {
        Self::with_clock(settings, Arc::new(DefaultClock))
    }

    pub fn with_clock(settings: RateLimitSettings, clock: Arc<dyn Clock>) -> Self {
        let window = chrono_duration(settings.window);
        Self {
            clock,
            settings,
            window,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub async fn check(&self, ip: IpAddr) -> Result<(), RateLimitError> {
        let now = self.clock.utc();
        let mut guard = self.entries.lock().await;
        let entry = guard.entry(ip).or_insert_with(|| RateEntry {
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
            return Err(RateLimitError::LimitExceeded { retry_in });
        }

        entry.count += 1;
        Ok(())
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

    #[tokio::test]
    async fn allows_requests_up_to_limit() -> Result<(), RateLimitError> {
        let settings = RateLimitSettings {
            max_requests: 2,
            window: Duration::from_secs(60),
        };
        let limiter =
            RateLimiter::with_clock(settings, ManualClock::from_times(timestamps(&[0, 0, 10])));
        let ip: IpAddr = "192.0.2.1"
            .parse()
            .map_err(|_| RateLimitError::LimitExceeded {
                retry_in: Duration::ZERO,
            })?;
        limiter.check(ip).await?;
        limiter.check(ip).await?;
        Ok(())
    }

    #[tokio::test]
    async fn enforces_limit() {
        let settings = RateLimitSettings {
            max_requests: 1,
            window: Duration::from_secs(60),
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
        let settings = RateLimitSettings {
            max_requests: 1,
            window: Duration::from_secs(10),
        };
        let limiter =
            RateLimiter::with_clock(settings, ManualClock::from_times(timestamps(&[0, 15])));
        let ip: IpAddr = "203.0.113.5"
            .parse()
            .map_err(|_| RateLimitError::LimitExceeded {
                retry_in: Duration::ZERO,
            })?;
        limiter.check(ip).await?;
        limiter.check(ip).await?;
        Ok(())
    }
}
