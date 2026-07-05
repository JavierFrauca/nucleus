//! Per-IP token-bucket rate limiting — dependency-free.
//!
//! Off by default; enable with `NUCLEUS_RATE_LIMIT_RPM=<n>`. The limiter keys on
//! the **direct peer IP** (from the connection info) unless `NUCLEUS_TRUST_PROXY=true`,
//! in which case it uses the first `X-Forwarded-For` address instead. Only enable
//! that behind a proxy you control that overwrites/strips the header from
//! clients — otherwise any caller can spoof it to dodge the per-IP budget. Burst
//! capacity equals the per-minute budget; tokens refill continuously at `rpm/60`
//! per second.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token-bucket limiter keyed by client IP. Cheap: one mutex-guarded map, no
/// background task — buckets refill lazily on access and idle ones are pruned
/// once the map grows large.
pub struct RateLimit {
    rpm: u32,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimit {
    pub fn new(rpm: u32) -> Self {
        Self {
            rpm,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Try to admit one request from `ip`. `true` = allowed.
    pub fn allow(&self, ip: IpAddr) -> bool {
        if self.rpm == 0 {
            return true; // disabled
        }
        let cap = self.rpm as f64;
        let now = Instant::now();
        let mut map = self.buckets.lock().unwrap();
        // Keep the map bounded under a flood of distinct source IPs: drop buckets
        // that have fully refilled (i.e. callers who've gone quiet).
        if map.len() > 10_000 {
            map.retain(|_, b| b.tokens < cap);
        }
        let b = map.entry(ip).or_insert(Bucket {
            tokens: cap,
            last: now,
        });
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.last = now;
        b.tokens = (b.tokens + elapsed * (cap / 60.0)).min(cap);
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The client IP from the connection info, or an unspecified address when it is
/// absent (e.g. in tests that drive the router without a real peer).
///
/// When `trust_proxy` is `true`, the first address in `X-Forwarded-For` is used
/// instead (falling back to the connection info if the header is absent or
/// unparsable). This must stay opt-in: the header is caller-supplied, so trusting
/// it without a proxy that overwrites/strips it from clients lets anyone spoof
/// their way past the per-IP budget.
fn client_ip(req: &Request, trust_proxy: bool) -> IpAddr {
    if trust_proxy {
        if let Some(ip) = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .and_then(|v| v.parse::<IpAddr>().ok())
        {
            return ip;
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// Axum middleware enforcing `limiter`. Over-budget requests get `429` with a
/// JSON body, matching the rest of the API's error shape.
pub async fn enforce(
    limiter: Arc<RateLimit>,
    trust_proxy: bool,
    req: Request,
    next: Next,
) -> Response {
    if limiter.allow(client_ip(&req, trust_proxy)) {
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
    fn allows_burst_then_blocks() {
        let rl = RateLimit::new(2);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(rl.allow(ip));
        assert!(rl.allow(ip));
        assert!(!rl.allow(ip), "third immediate request exceeds the burst");
    }

    #[test]
    fn zero_rpm_is_disabled() {
        let rl = RateLimit::new(0);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..1000 {
            assert!(rl.allow(ip));
        }
    }

    #[test]
    fn buckets_are_per_ip() {
        let rl = RateLimit::new(1);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        assert!(rl.allow(a));
        assert!(rl.allow(b), "a different IP has its own budget");
        assert!(!rl.allow(a), "the first IP is now spent");
    }

    fn req_with_xff(xff: Option<&str>) -> Request {
        let mut b = Request::builder().uri("/");
        if let Some(v) = xff {
            b = b.header("x-forwarded-for", v);
        }
        b.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn xff_ignored_by_default() {
        let req = req_with_xff(Some("203.0.113.7"));
        assert_eq!(
            client_ip(&req, false),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            "without trust_proxy, a caller-supplied header must not be trusted"
        );
    }

    #[test]
    fn xff_trusted_when_opted_in() {
        let req = req_with_xff(Some("203.0.113.7, 10.0.0.1"));
        assert_eq!(
            client_ip(&req, true),
            "203.0.113.7".parse::<IpAddr>().unwrap(),
            "takes the first (client) address, not the intermediate proxies"
        );
    }

    #[test]
    fn xff_missing_falls_back_to_connect_info() {
        let req = req_with_xff(None);
        assert_eq!(
            client_ip(&req, true),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            "no header and no ConnectInfo in this bare request -> unspecified fallback"
        );
    }

    #[test]
    fn xff_malformed_falls_back() {
        let req = req_with_xff(Some("not-an-ip"));
        assert_eq!(client_ip(&req, true), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
}
