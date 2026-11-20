use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

/// Async-friendly Token Bucket bandwidth limiter.
/// `rate_bps == 0` disables throttling completely with near-zero atomic overhead.
pub struct TokenBucket {
    rate_bps: AtomicU64,
    state: Mutex<BucketState>,
}

impl TokenBucket {
    pub fn new(rate_bps: u64) -> Self {
        let cap = Self::capacity_for(rate_bps);
        Self {
            rate_bps: AtomicU64::new(rate_bps),
            state: Mutex::new(BucketState {
                tokens: cap,
                last_refill: Instant::now(),
            }),
        }
    }

    fn capacity_for(rate: u64) -> f64 {
        if rate == 0 {
            0.0
        } else {
            // 100ms pacing window (16 KB .. 256 KB) keeps throttled transfers smooth
            // without coarse stop-and-go bursts while staying syscall-efficient.
            ((rate as f64) * 0.10).clamp(16_384.0, 262_144.0)
        }
    }

    pub fn set_rate(&self, rate_bps: u64) {
        self.rate_bps.store(rate_bps, Ordering::Relaxed);
        if let Ok(mut guard) = self.state.lock() {
            let cap = Self::capacity_for(rate_bps);
            guard.tokens = guard.tokens.min(cap);
            guard.last_refill = Instant::now();
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate_bps.load(Ordering::Relaxed)
    }

    /// Consume `bytes` tokens, asynchronously sleeping if the bucket is depleted.
    pub async fn consume(&self, bytes: u64) {
        let mut remaining = bytes as f64;
        while remaining > 0.0 {
            let rate = self.rate_bps.load(Ordering::Relaxed);
            if rate == 0 {
                return;
            }

            let sleep_duration = {
                let mut guard = self.state.lock().unwrap();
                let now = Instant::now();
                let elapsed = now.duration_since(guard.last_refill).as_secs_f64();
                guard.last_refill = now;

                let cap = Self::capacity_for(rate);
                guard.tokens = (guard.tokens + elapsed * (rate as f64)).min(cap);

                // Take up to one bucket burst worth of tokens; negative balance tracks exact debt
                // across concurrent workers so aggregate throughput matches `rate` accurately.
                let take = remaining.min(cap);
                guard.tokens -= take;
                remaining -= take;
                if guard.tokens >= 0.0 {
                    None
                } else {
                    let secs = (-guard.tokens / (rate as f64)).clamp(0.002, 0.10);
                    Some(Duration::from_secs_f64(secs))
                }
            };

            if let Some(dur) = sleep_duration {
                tokio::time::sleep(dur).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_token_bucket_unlimited_is_instant() {
        let bucket = TokenBucket::new(0);
        let start = Instant::now();
        bucket.consume(10_000_000).await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_token_bucket_throttles_accurately() {
        // 200 KB/s rate -> initial burst ~50 KB; consuming 150 KB should take ~0.4-0.6s
        let bucket = TokenBucket::new(200_000);
        let start = Instant::now();
        for _ in 0..10 {
            bucket.consume(15_000).await;
        }
        let elapsed = start.elapsed().as_secs_f64();
        assert!(elapsed >= 0.35, "expected throttling >= 0.35s, got {:.3}s", elapsed);
        assert!(elapsed <= 1.20, "expected throttling <= 1.20s, got {:.3}s", elapsed);
    }
}
