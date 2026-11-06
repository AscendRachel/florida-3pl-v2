//! Hardening that applies to every request: browser security headers and a check against cross-site requests.

use axum::extract::Request;
use axum::http::header::{HeaderName, HeaderValue, HOST, ORIGIN};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

/// What the browser may load. The pages have a few small inline scripts and styles, so those are allowed; everything else only
/// from this site. Pictures may come from anywhere (product image links), and label files are shown from Google Drive.
const CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src * data: blob:; \
    font-src 'self' data:; connect-src 'self'; frame-src https://drive.google.com blob:; worker-src 'self' blob:; object-src 'none'; \
    base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

/// True when HTTPS is certain: the HTTPS_ONLY setting, or the proxy (Caddy) says the visitor came over https.
pub fn https_only() -> bool {
    matches!(std::env::var("HTTPS_ONLY").as_deref(), Ok("1") | Ok("true") | Ok("yes"))
}

fn host_of_origin(origin: &str) -> &str {
    origin.split_once("://").map_or(origin, |(_, rest)| rest).split('/').next().unwrap_or("")
}

pub async fn harden(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let h = req.headers();
    let via_https = https_only() || h.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");

    // A request that changes something must come from this site's own pages. SameSite cookies already stop most of it;
    // this refuses the rest (a browser tells the site the page that sent the request in `Origin`).
    if !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS) {
        let host = h.get("x-forwarded-host").or_else(|| h.get(HOST)).and_then(|v| v.to_str().ok()).unwrap_or("");
        let cross = match h.get(ORIGIN).and_then(|v| v.to_str().ok()) {
            Some(o) => o.eq_ignore_ascii_case("null") || !host_of_origin(o).eq_ignore_ascii_case(host),
            None => h.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("cross-site"),
        };
        if cross {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": "This request did not come from the app's own page." }))).into_response();
        }
    }

    let mut res = next.run(req).await;
    let hd = res.headers_mut();
    let mut put = |k: &'static str, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            hd.insert(HeaderName::from_static(k), v);
        }
    };
    put("x-content-type-options", "nosniff");
    put("x-frame-options", "DENY");
    put("referrer-policy", "same-origin");
    put("permissions-policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=()");
    put("cross-origin-opener-policy", "same-origin");
    put("cross-origin-resource-policy", "same-origin");
    put("content-security-policy", CSP);
    if via_https {
        put("strict-transport-security", "max-age=31536000; includeSubDomains");
    }
    res
}
