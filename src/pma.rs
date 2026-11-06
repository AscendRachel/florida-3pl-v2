//! The gate in front of phpMyAdmin (https://DOMAIN/phpmyadmin).
//!
//! Caddy asks `/api/pma-auth` before it lets any request through to phpMyAdmin. The answer is yes only when the browser holds a
//! valid JWT (HS256, signed with JWT_SECRET from `.env`) that was handed to a signed-in admin by `/pma-login`, and that person is
//! still an active admin. Without it, the browser is sent to `/pma-login`, which sends people who are not signed in to the sign-in page.
//! phpMyAdmin then still asks for the database user and password.

use axum::extract::State;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::error::AppResult;
use crate::{auth, AppState};

pub const COOKIE_NAME: &str = "f3pl_pma";
const PATH: &str = "/phpmyadmin";
const TTL_SECONDS: i64 = 8 * 3600;
const AUDIENCE: &str = "phpmyadmin";

/// The signing key, or None when JWT_SECRET is missing or shorter than 32 characters (then the gate stays shut).
fn key() -> Option<Vec<u8>> {
    std::env::var("JWT_SECRET").ok().map(|s| s.trim().to_string()).filter(|s| s.len() >= 32).map(String::into_bytes)
}

fn sign(data: &str, key: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

pub fn make_token(sub: &str, key: &[u8], now: i64) -> String {
    let head = B64.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let body = B64.encode(json!({ "sub": sub, "aud": AUDIENCE, "iat": now, "exp": now + TTL_SECONDS }).to_string());
    let data = format!("{head}.{body}");
    format!("{data}.{}", B64.encode(sign(&data, key)))
}

/// The person (`sub`) when the token is ours, unaltered and not expired.
pub fn check_token(token: &str, key: &[u8], now: i64) -> Option<String> {
    let mut parts = token.split('.');
    let (head, body, sig) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let header: Value = serde_json::from_slice(&B64.decode(head).ok()?).ok()?;
    if header["alg"] != "HS256" {
        return None; // never trust the token's own idea of how it is signed
    }
    let given = B64.decode(sig).ok()?;
    if !bool::from(sign(&format!("{head}.{body}"), key).ct_eq(&given)) {
        return None;
    }
    let claims: Value = serde_json::from_slice(&B64.decode(body).ok()?).ok()?;
    if claims["aud"] != AUDIENCE || claims["exp"].as_i64()? <= now {
        return None;
    }
    claims["sub"].as_str().map(str::to_string)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn cookie(value: &str, max_age: i64) -> String {
    format!("{COOKIE_NAME}={value}; Max-Age={max_age}; Path={PATH}; HttpOnly; SameSite=Lax; Secure")
}

/// The cookie that removes the pass (sent on sign-out).
pub fn clear_cookie() -> String {
    cookie("", 0)
}

fn token_in(headers: &HeaderMap) -> Option<String> {
    let header = headers.get(COOKIE)?.to_str().ok()?;
    header.split(';').filter_map(|c| c.trim().strip_prefix(COOKIE_NAME)?.strip_prefix('=')).map(str::to_string).next()
}

/// GET /pma-login: a signed-in admin gets the pass and is sent on to phpMyAdmin; everybody else is turned away.
pub async fn login(State(s): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let Some(key) = key() else {
        return Ok((StatusCode::SERVICE_UNAVAILABLE, "phpMyAdmin access is not set up: JWT_SECRET is missing in the server's .env.").into_response());
    };
    let user = match auth::token_of(&headers) {
        Some(t) => auth::session_user(&s.db, &t).await?,
        None => None,
    };
    let Some(user) = user else { return Ok(Redirect::to("/login").into_response()) };
    if !user.is_admin() {
        tracing::warn!(target: "security", "phpMyAdmin pass refused (not an admin) user={}", user.username);
        return Ok((StatusCode::FORBIDDEN, "Only admins can open phpMyAdmin.").into_response());
    }
    tracing::info!(target: "security", "phpMyAdmin pass given to {}", user.username);
    let token = make_token(&user.username, &key, now());
    Ok((AppendHeaders([(SET_COOKIE, cookie(&token, TTL_SECONDS))]), Redirect::to("/phpmyadmin/")).into_response())
}

/// GET /api/pma-auth: what Caddy asks before every request to phpMyAdmin. 200 lets it through; anything else goes back to the visitor.
pub async fn check(State(s): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let deny = || Redirect::to("/pma-login").into_response();
    let Some(key) = key() else { return Ok(deny()) };
    let Some(sub) = token_in(&headers).and_then(|t| check_token(&t, &key, now())) else { return Ok(deny()) };
    // the person must still be an active admin (removing an admin shuts the door at once)
    let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = ? AND disabled = 0 AND role = 'admin'").bind(&sub).fetch_one(&s.db).await?;
    Ok(if still > 0 { StatusCode::OK.into_response() } else { deny() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn a_fresh_token_is_accepted() {
        assert_eq!(check_token(&make_token("rachel", KEY, 1000), KEY, 1001).as_deref(), Some("rachel"));
    }

    #[test]
    fn expired_wrong_key_and_tampered_tokens_are_refused() {
        let t = make_token("rachel", KEY, 1000);
        assert!(check_token(&t, KEY, 1000 + TTL_SECONDS + 1).is_none());
        assert!(check_token(&t, b"another-secret-another-secret-123", 1001).is_none());
        let mut parts: Vec<&str> = t.split('.').collect();
        let forged = B64.encode(json!({ "sub": "admin2", "aud": AUDIENCE, "iat": 1000, "exp": 99999999999i64 }).to_string());
        parts[1] = &forged;
        assert!(check_token(&parts.join("."), KEY, 1001).is_none());
        assert!(check_token("a.b", KEY, 1001).is_none());
        assert!(check_token("", KEY, 1001).is_none());
    }

    #[test]
    fn alg_none_is_refused() {
        let head = B64.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let body = B64.encode(json!({ "sub": "x", "aud": AUDIENCE, "exp": 99999999999i64 }).to_string());
        assert!(check_token(&format!("{head}.{body}."), KEY, 1).is_none());
    }
}
