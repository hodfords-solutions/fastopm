//! got's default retry rules (got 16: `core/options.js` defaults and
//! `core/calculate-retry-delay.js`), which Verdaccio uses with only `limit` changed.
//!
//! - every uplink request is a `GET`, which is retryable;
//! - retryable statuses: 408, 413, 429, 500, 502, 503, 504, 521, 522, 524;
//! - retryable network errors: `ETIMEDOUT`, `ECONNRESET`, `EADDRINUSE`, `ECONNREFUSED`, `EPIPE`,
//!   `ENOTFOUND`, `ENETUNREACH`, `EAI_AGAIN`;
//! - with a `Retry-After` header on a retryable status: wait that long, or give up when it is
//!   longer than the request timeout (`maxRetryAfter` defaults to `timeout.request`);
//! - otherwise 413 is not retried;
//! - otherwise wait `2^(attempt - 1)` seconds plus up to 100 ms of noise.

use std::error::Error as StdError;
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Statuses got retries.
pub(crate) const RETRY_STATUS_CODES: [u16; 10] = [408, 413, 429, 500, 502, 503, 504, 521, 522, 524];

/// The Node.js error code of a failed attempt, as far as got's rules and Verdaccio's
/// `_isRequestTimeout` care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetCode {
    /// `ETIMEDOUT` (got's own timeout, or the OS one).
    TimedOut,
    /// `ECONNRESET`: connection reset, or closed before the response completed
    /// (Node.js "socket hang up").
    ConnReset,
    /// `EADDRINUSE`.
    AddrInUse,
    /// `ECONNREFUSED`.
    ConnRefused,
    /// `EPIPE`.
    Pipe,
    /// `ENOTFOUND` / `EAI_AGAIN`: name resolution failed.
    Dns,
    /// `ENETUNREACH`.
    NetUnreachable,
    /// Anything else (TLS, invalid response...): not retried.
    Other,
}

impl NetCode {
    /// Whether got retries this error.
    pub(crate) fn is_retryable(self) -> bool {
        !matches!(self, NetCode::Other)
    }

    /// Verdaccio's `_isRequestTimeout` (for errors without a response).
    pub(crate) fn is_timeout(self) -> bool {
        matches!(self, NetCode::TimedOut | NetCode::ConnReset)
    }

    /// Classify a reqwest error by walking its source chain.
    pub(crate) fn of(err: &reqwest::Error) -> NetCode {
        if err.is_timeout() {
            return NetCode::TimedOut;
        }
        let mut source: Option<&(dyn StdError + 'static)> = Some(err);
        while let Some(e) = source {
            if let Some(io_err) = e.downcast_ref::<io::Error>() {
                match io_err.kind() {
                    io::ErrorKind::ConnectionRefused => return NetCode::ConnRefused,
                    io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::UnexpectedEof => return NetCode::ConnReset,
                    io::ErrorKind::BrokenPipe => return NetCode::Pipe,
                    io::ErrorKind::AddrInUse => return NetCode::AddrInUse,
                    io::ErrorKind::NetworkUnreachable => return NetCode::NetUnreachable,
                    io::ErrorKind::TimedOut => return NetCode::TimedOut,
                    _ => {}
                }
            }
            let text = e.to_string();
            // hyper's `IncompleteMessage`: the peer closed the connection before answering.
            if text.contains("connection closed before message completed") {
                return NetCode::ConnReset;
            }
            // hyper-util's connector wraps resolver failures as "dns error: ...".
            if text.starts_with("dns error") {
                return NetCode::Dns;
            }
            source = e.source();
        }
        NetCode::Other
    }
}

/// `Retry-After` in milliseconds as got reads it: digits are seconds, a non-numeric value is a
/// date (only the IMF-fixdate form is understood here; got's other date forms are rare),
/// anything else is ignored. Past dates give 0.
pub(crate) fn parse_retry_after(value: &str, now: SystemTime) -> Option<u64> {
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return Some(value.parse::<u64>().unwrap_or(u64::MAX).saturating_mul(1000));
    }
    let at_ms = i128::from(parse_imf_fixdate(value)?) * 1000;
    let now_ms = now.duration_since(UNIX_EPOCH).ok()?.as_millis() as i128;
    Some((at_ms - now_ms).clamp(0, i128::from(u64::MAX)) as u64)
}

/// Seconds since the epoch for `Sun, 06 Nov 1994 08:49:37 GMT`.
fn parse_imf_fixdate(value: &str) -> Option<i64> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    let [_weekday, day, month, year, time, "GMT"] = parts.as_slice() else {
        return None;
    };
    let day: i64 = day.parse().ok()?;
    let year: i64 = year.parse().ok()?;
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS.iter().position(|m| m == month)? as i64 + 1;
    let mut hms = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, s) = (hms.next()??, hms.next()??, hms.next()??);
    if hms.next().is_some() || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + m * 60 + s)
}

/// Why one attempt failed, as seen by the retry logic.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Failure {
    /// Non-2xx status, with the `Retry-After` delay in ms if the response had a usable one.
    Status { code: u16, retry_after_ms: Option<u64> },
    /// No (complete) response.
    Net(NetCode),
}

/// Delay before the next attempt, `None` to give up.
///
/// `attempt` counts the attempt that just failed, from 1 (got's `attemptCount`). `unit` is
/// one second in production (tests shrink it); `noise` is a random number in `[0, 1)`.
pub(crate) fn retry_delay(
    attempt: u32,
    limit: u32,
    failure: Failure,
    request_timeout: Duration,
    unit: Duration,
    noise: f64,
) -> Option<Duration> {
    if attempt > limit {
        return None;
    }
    match failure {
        Failure::Net(code) if !code.is_retryable() => return None,
        Failure::Status { code, .. } if !RETRY_STATUS_CODES.contains(&code) => return None,
        Failure::Status { retry_after_ms: Some(ms), .. } => {
            // `retryAfter > computedValue ? 0 : Math.max(1, retryAfter)`
            return if u128::from(ms) > request_timeout.as_millis() {
                None
            } else {
                Some(Duration::from_millis(ms.max(1)))
            };
        }
        Failure::Status { code: 413, .. } => return None,
        _ => {}
    }
    let factor = 2u32.saturating_pow(attempt - 1);
    let base = unit.saturating_mul(factor);
    let noise = unit.mul_f64(0.1 * noise.clamp(0.0, 1.0));
    Some((base + noise).max(Duration::from_millis(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: Duration = Duration::from_secs(1);
    const T30: Duration = Duration::from_secs(30);

    fn status(code: u16) -> Failure {
        Failure::Status { code, retry_after_ms: None }
    }

    #[test]
    fn exponential_backoff_with_noise() {
        assert_eq!(retry_delay(1, 2, status(500), T30, SEC, 0.0), Some(SEC));
        assert_eq!(
            retry_delay(2, 2, status(503), T30, SEC, 0.5),
            Some(Duration::from_millis(2050))
        );
        assert_eq!(retry_delay(3, 2, status(500), T30, SEC, 0.0), None);
        assert_eq!(retry_delay(1, 0, status(500), T30, SEC, 0.0), None);
    }

    #[test]
    fn only_got_statuses_and_error_codes_are_retried() {
        for code in RETRY_STATUS_CODES {
            let expected = code != 413;
            assert_eq!(
                retry_delay(1, 2, status(code), T30, SEC, 0.0).is_some(),
                expected,
                "{code}"
            );
        }
        for code in [400, 401, 403, 404, 501, 505] {
            assert_eq!(retry_delay(1, 2, status(code), T30, SEC, 0.0), None);
        }
        for code in [
            NetCode::TimedOut,
            NetCode::ConnReset,
            NetCode::ConnRefused,
            NetCode::Dns,
            NetCode::Pipe,
            NetCode::AddrInUse,
            NetCode::NetUnreachable,
        ] {
            assert!(retry_delay(1, 2, Failure::Net(code), T30, SEC, 0.0).is_some());
        }
        assert_eq!(retry_delay(1, 2, Failure::Net(NetCode::Other), T30, SEC, 0.0), None);
    }

    #[test]
    fn retry_after_is_used_unless_longer_than_the_timeout() {
        let with = |code, ms| Failure::Status { code, retry_after_ms: Some(ms) };
        assert_eq!(retry_delay(1, 2, with(429, 3000), T30, SEC, 0.9), Some(Duration::from_secs(3)));
        assert_eq!(retry_delay(1, 2, with(429, 31_000), T30, SEC, 0.0), None);
        // Checked before the 413 rule; zero becomes the 1 ms minimum.
        assert_eq!(retry_delay(1, 2, with(413, 0), T30, SEC, 0.0), Some(Duration::from_millis(1)));
        // Not consulted for statuses got does not retry.
        assert_eq!(retry_delay(1, 2, with(404, 10), T30, SEC, 0.0), None);
    }

    #[test]
    fn retry_after_parsing() {
        let now = UNIX_EPOCH + Duration::from_secs(784_111_777); // Sun, 06 Nov 1994 08:49:37 GMT
        assert_eq!(parse_retry_after("5", now), Some(5000));
        assert_eq!(parse_retry_after("1.5", now), None);
        assert_eq!(parse_retry_after("", now), None);
        assert_eq!(parse_retry_after("Sun, 06 Nov 1994 08:49:47 GMT", now), Some(10_000));
        assert_eq!(parse_retry_after("Sun, 06 Nov 1994 08:49:00 GMT", now), Some(0));
        assert_eq!(parse_retry_after("soon", now), None);
    }
}
