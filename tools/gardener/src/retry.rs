use crate::errors::GardenerError;
use crate::logging::append_run_log;
use serde_json::json;
use std::time::Duration;

pub struct RetryConfig {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub operation_name: &'static str,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(15),
            operation_name: "operation",
        }
    }
}

pub fn retry_with_backoff<T, F>(config: &RetryConfig, mut f: F) -> Result<T, GardenerError>
where
    F: FnMut() -> Result<T, GardenerError>,
{
    let mut last_err = None;

    for attempt in 0..config.max_attempts.max(1) {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) => {
                append_run_log(
                    "warn",
                    "retry.attempt_failed",
                    json!({
                        "operation": config.operation_name,
                        "attempt": attempt + 1,
                        "max_attempts": config.max_attempts.max(1),
                        "error": err.to_string()
                    }),
                );
                last_err = Some(err);
                if attempt + 1 < config.max_attempts.max(1) {
                    let delay_ms = (config.base_delay.as_millis() as u64)
                        .saturating_mul(attempt as u64 + 1)
                        .min(config.max_delay.as_millis() as u64);
                    std::thread::sleep(Duration::from_millis(delay_ms));
                }
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| GardenerError::Process("retry failed without attempts".to_string())))
}

#[cfg(test)]
mod tests {
    use super::{retry_with_backoff, RetryConfig};
    use crate::errors::GardenerError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn retry_with_backoff_returns_immediately_on_success() {
        let attempts = AtomicUsize::new(0);
        let config = RetryConfig {
            max_attempts: 3,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            operation_name: "immediate_success",
        };

        let result = retry_with_backoff(&config, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok::<_, GardenerError>(42)
        })
        .expect("should succeed");

        assert_eq!(result, 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_with_backoff_succeeds_after_retry() {
        let attempts = AtomicUsize::new(0);
        let config = RetryConfig {
            max_attempts: 3,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            operation_name: "retry_success",
        };

        let result = retry_with_backoff(&config, || {
            let current = attempts.fetch_add(1, Ordering::SeqCst);
            if current == 0 {
                Err(GardenerError::Process("transient".to_string()))
            } else {
                Ok::<_, GardenerError>("ok")
            }
        })
        .expect("should succeed");

        assert_eq!(result, "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn retry_with_backoff_returns_last_error_when_exhausted() {
        let attempts = AtomicUsize::new(0);
        let config = RetryConfig {
            max_attempts: 3,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            operation_name: "retry_exhausted",
        };

        let err = retry_with_backoff(&config, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(GardenerError::Process("still failing".to_string()))
        })
        .expect_err("should fail");

        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(err.to_string().contains("still failing"));
    }
}
