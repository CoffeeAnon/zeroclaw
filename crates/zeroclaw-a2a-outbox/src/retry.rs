//! Exponential backoff retry policy for outbox deliveries.

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub factor: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 5, base_delay: Duration::from_secs(1), factor: 4 }
    }
}

impl RetryPolicy {
    /// Returns the delay before the `attempt`-th retry (0-indexed).
    /// Returns `None` when the attempt count exceeds `max_attempts`.
    pub fn delay_for(&self, attempt: u32) -> Option<Duration> {
        if attempt >= self.max_attempts {
            return None;
        }
        let pow = self.factor.checked_pow(attempt)?;
        Some(self.base_delay * pow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_gives_expected_schedule() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for(0), Some(Duration::from_secs(1)));
        assert_eq!(p.delay_for(1), Some(Duration::from_secs(4)));
        assert_eq!(p.delay_for(2), Some(Duration::from_secs(16)));
        assert_eq!(p.delay_for(3), Some(Duration::from_secs(64)));
        assert_eq!(p.delay_for(4), Some(Duration::from_secs(256)));
        assert_eq!(p.delay_for(5), None, "exhausted");
    }

    #[test]
    fn custom_policy_respects_max_attempts() {
        let p = RetryPolicy { max_attempts: 2, base_delay: Duration::from_millis(10), factor: 2 };
        assert!(p.delay_for(0).is_some());
        assert!(p.delay_for(1).is_some());
        assert!(p.delay_for(2).is_none());
    }

    #[test]
    fn factor_overflow_returns_none() {
        let p = RetryPolicy { max_attempts: 100, base_delay: Duration::from_secs(1), factor: u32::MAX };
        assert_eq!(p.delay_for(3), None, "overflow must not panic");
    }
}
