//! Lightweight in-memory rate limiting.
//!
//! A per-client **token bucket**: each client (keyed by source IP) gets a bucket
//! of `burst` tokens that refills at `rps` tokens per second. Each request spends
//! one token; when the bucket is empty the request is shed with `429 Too Many
//! Requests`. State lives in process (a `Mutex<HashMap>`), so it is per-node — a
//! pragmatic guard against a single misbehaving client, not a distributed quota.
//! Off by default; enabled via `NUCLEUS_RATE_LIMIT_RPS`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// One client's bucket.
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token-bucket rate limiter shared across handlers.
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    /// Sustained refill rate (tokens per second).
    rate: f64,
    /// Bucket capacity (max burst).
    burst: f64,
}

impl RateLimiter {
    /// Build a limiter allowing `rps` sustained requests per second per client
    /// with up to `burst` in a spike. `burst` is floored to at least 1.
    pub fn new(rps: f64, burst: f64) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            rate: rps.max(0.0),
            burst: burst.max(1.0),
        }
    }

    /// Try to admit one request from `key`. Returns `true` if allowed (a token
    /// was available), `false` if the client is over budget.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = match self.buckets.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(), // a poisoned lock shouldn't deny traffic
        };
        let bucket = map.entry(key.to_string()).or_insert(Bucket {
            tokens: self.burst,
            last: now,
        });
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Resolve the client key for a request: the peer IP (from `ConnectInfo`), else
/// the first `X-Forwarded-For` hop (behind a proxy), else a shared `"global"`
/// bucket so the limiter still functions when neither is available.
fn client_key(req: &Request) -> String {
    if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return addr.ip().to_string();
    }
    if let Some(fwd) = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(first) = fwd.split(',').next() {
            let first = first.trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "global".to_string()
}

/// Axum middleware enforcing [`RateLimiter`]. Wire it with
/// `from_fn_with_state(limiter, rate_limit)`.
pub async fn rate_limit(
    State(limiter): State<Arc<RateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    if limiter.check(&client_key(&req)) {
        next.run(req).await
    } else {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limit exceeded" })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_admits_burst_then_sheds() {
        // 0 rps refill, burst of 2: exactly two requests pass, the third is shed.
        let rl = RateLimiter::new(0.0, 2.0);
        assert!(rl.check("a"));
        assert!(rl.check("a"));
        assert!(!rl.check("a"));
        // A different client has its own independent bucket.
        assert!(rl.check("b"));
    }

    #[test]
    fn tokens_refill_over_time() {
        // High refill rate: after spending the burst, a token is back almost
        // immediately.
        let rl = RateLimiter::new(1000.0, 1.0);
        assert!(rl.check("a"));
        assert!(!rl.check("a"));
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(rl.check("a"), "a token should have refilled");
    }
}
