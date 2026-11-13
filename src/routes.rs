//! The web addresses of the app. Phase 2: the pages, sign-in, who am I, the team, branding, and the health and change counters.
//! The addresses of the other tabs are added in the next phases, one tab at a time, answering the same way the Python app did.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{AppendHeaders, Html, IntoResponse, Json, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::{self, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::{clock, AppState};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_FAILS: usize = 8;
const MAX_FAILS_PER_USER: usize = 30;
const FAIL_WINDOW: Duration = Duration::from_secs(600);

pub fn router(state: AppState) -> Router {
    let web = state.config.web_dir.clone();
    Router::new()
        .route("/", get(index_page))
        .route("/login", get(login_page))
        .nest_service("/static", ServeDir::new(format!("{web}/static")))
        .route("/pma-login", get(crate::pma::login))
        .route("/api/pma-auth", get(crate::pma::check))
        .route("/api/health", get(health))
        .route("/api/build", get(build))
        .route("/api/branding", get(branding))
        .route("/api/setup-needed", get(setup_needed))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/version", get(version))
        .route("/api/me", get(me))
        .route("/api/me/password", post(change_password))
        .route("/api/timeframe", get(crate::timeframe::list))
        .route("/api/timeframe/resolve", post(crate::timeframe::resolve))
        .route("/api/timeframe/reopen", post(crate::timeframe::reopen))
        .route("/api/team", get(team_list))
        .route("/api/team/:action", post(team_action))
        .route("/api/roles/:action", post(roles_action))
        .route("/api/meta", get(crate::orders_api::meta))
        .route("/api/orders", get(crate::orders_api::orders_get).post(crate::orders_api::orders))
        .route("/api/values", get(crate::orders_api::values_get).post(crate::orders_api::values))
        .route("/api/colors", get(crate::orders_api::colors_get).post(crate::orders_api::colors))
        .route("/api/duplicates", get(crate::orders_api::duplicates))
        .route("/api/history", get(crate::orders_api::history))
        .route("/api/ship-events", get(crate::orders_api::ship_events))
        .route("/api/views", get(crate::orders_api::views_get))
        .route("/api/scan", post(crate::scan::scan_root))
        .route("/api/scan/holds", get(crate::scan::holds))
        .route("/api/scan/recent", get(crate::scan::recent))
        .route("/api/scan/samples", get(crate::scan::samples))
        .route("/api/scan/:action", post(crate::scan::scan_post))
        .route("/api/label", get(crate::scan::label))
        .route("/api/returns/label", get(crate::scan::return_label))
        .route("/api/chat", get(crate::team_api::chat))
        .route("/api/chat/send", post(crate::team_api::chat_send))
        .route("/api/worklog", get(crate::team_api::worklog))
        .route("/api/branding", post(crate::team_api::branding_save))
        .route("/branding/logo", get(crate::team_api::branding_logo))
        .route("/api/drive/names", post(crate::team_api::drive_names))
        .route("/api/admin/labels", get(crate::team_api::admin_labels))
        .route("/api/admin/labels/approve", post(crate::team_api::label_approve))
        .route("/api/admin/cards", get(crate::team_api::admin_cards))
        .route("/api/admin/daily", get(crate::team_api::admin_daily))
        .route("/api/checklist", get(crate::checklist::get))
        .route("/api/checklist/mark", post(crate::checklist::mark))
        .route("/api/checklist/number", post(crate::checklist::number))
        .route("/api/checklist/cell", post(crate::checklist::cell))
        .route("/api/checklist/task", post(crate::checklist::task_edit))
        .route("/api/checklist/agents", post(crate::checklist::agents_edit))
        .route("/api/materials", get(crate::returns::materials_get))
        .route("/api/materials/:action", post(crate::returns::materials_post))
        .route("/api/kohls", get(crate::returns::kohls_get))
        .route("/api/kohls/:action", post(crate::returns::kohls_post))
        .route("/api/returns", get(crate::returns::returns_get))
        .route("/api/returns/:action", post(crate::returns::returns_post))
        .route("/api/amazon/preview", post(crate::amazon::preview).layer(axum::extract::DefaultBodyLimit::max(300 * 1024 * 1024)))
        .route("/api/amazon/apply", post(crate::amazon::apply))
        .route("/api/upload", post(crate::sheet_import::upload).layer(axum::extract::DefaultBodyLimit::max(crate::sheet_import::MAX_UPLOAD)))
        .route("/api/upload-link", post(crate::sheet_import::upload_link))
        .route("/api/import", post(crate::sheet_import::run_import))
        .route("/api/progress", get(crate::sheet_import::progress))
        .route("/api/cells", post(crate::orders_write::cells).layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)))
        .route("/api/dropdown", post(crate::admin_api::dropdown))
        .route("/api/locks/set", post(crate::admin_api::locks_set))
        .route("/api/views/:action", post(crate::admin_api::views))
        .route("/api/columns/:action", post(crate::admin_api::columns))
        .route("/api/rows/:action", post(crate::orders_write::rows))
        .route("/api/trash", get(crate::orders_write::trash))
        .route("/api/notify", get(crate::notify::feed))
        .route("/api/notify/seen", post(crate::notify::seen))
        .route("/api/dashboard", get(crate::dashboard::dashboard))
        .route("/api/dashboard/operators", post(crate::dashboard::save_operators))
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(crate::security::harden))
        .with_state(state)
}

// ---------------------------------------------------------------- pages

async fn page(state: &AppState, name: &str) -> AppResult<Response> {
    let body = tokio::fs::read_to_string(format!("{}/{name}", state.config.web_dir)).await?;
    Ok(([(CONTENT_TYPE, "text/html; charset=utf-8"), (CACHE_CONTROL, "no-store")], Html(body)).into_response())
}

/// The app itself: only for a signed-in person, otherwise on to the sign-in page.
async fn index_page(State(s): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let signed_in = match auth::token_of(&headers) {
        Some(t) => auth::session_user(&s.db, &t).await?.is_some(),
        None => false,
    };
    if !signed_in {
        return Ok(Redirect::to("/login").into_response());
    }
    page(&s, "index.html").await
}

async fn login_page(State(s): State<AppState>) -> AppResult<Response> {
    page(&s, "login.html").await
}

// ---------------------------------------------------------------- small public answers

/// Is the program up, is the database answering, and how much is in it.
async fn health(State(s): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let fail = |e: sqlx::Error| {
        tracing::error!("health check: {e}");
        (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "ok": false })))
    };
    let tables: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE()").fetch_one(&s.db).await.map_err(fail)?;
    // The numbers are only for someone on the server itself (no proxy in between); the internet only learns "ok".
    if headers.get("x-forwarded-for").is_none() && is_internal(&peer.ip()) {
        let orders: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orders").fetch_one(&s.db).await.map_err(fail)?;
        return Ok(Json(json!({ "ok": true, "app": APP_VERSION, "tables": tables, "orders": orders })));
    }
    Ok(Json(json!({ "ok": true })))
}

/// A short fingerprint of everything in web/, so open pages can tell when a new version of the app is ready.
fn build_id(dir: &std::path::Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut files = vec![];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    for p in files {
        if let Ok(m) = p.metadata() {
            (p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().to_string(), m.len(), m.modified().ok()).hash(&mut h);
        }
    }
    h.finish()
}

async fn build(State(s): State<AppState>) -> Json<Value> {
    let dir = std::path::PathBuf::from(&s.config.web_dir);
    let id = tokio::task::spawn_blocking(move || build_id(&dir)).await.unwrap_or(0);
    Json(json!({ "build": format!("{:012x}", id & 0xffff_ffff_ffff), "version": APP_VERSION }))
}

pub const DEFAULT_BRANDING: &str = r##"{"name":"Florida 3PL","initials":"F3","color":"#1f7a4d","logo":"","v":0}"##;

/// The app's name, colour and logo (the sign-in page shows it too).
async fn branding(State(s): State<AppState>) -> AppResult<Json<Value>> {
    let mut out: Value = serde_json::from_str(DEFAULT_BRANDING)?;
    let stored: Option<String> = sqlx::query_scalar("SELECT CAST(v AS CHAR) FROM app_settings WHERE k = 'branding'").fetch_optional(&s.db).await?;
    if let Some(Ok(Value::Object(m))) = stored.map(|t| serde_json::from_str::<Value>(&t)) {
        for (k, v) in m {
            out[k] = v;
        }
    }
    Ok(Json(out))
}

/// The count that every open page watches, to refresh itself when data changed.
async fn version(_u: CurrentUser, State(s): State<AppState>) -> Json<Value> {
    Json(json!({ "v": s.version.load(Ordering::Relaxed) }))
}

// ---------------------------------------------------------------- sign in

/// Loopback or a private network address (Docker's own network included): a proxy on the same machine, not the internet.
fn is_internal(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        std::net::IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Who is asking. X-Forwarded-For is believed only when it comes from our own proxy (Caddy), so nobody can pick a fake
/// address to get around the sign-in limit.
fn client_ip(headers: &HeaderMap, peer: &SocketAddr) -> String {
    if is_internal(&peer.ip()) {
        if let Some(f) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            return f.split(',').next().unwrap_or("").trim().chars().take(64).collect();
        }
    }
    peer.ip().to_string()
}

fn is_https(headers: &HeaderMap) -> bool {
    crate::security::https_only() || headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https")
}

fn text<'a>(b: &'a Value, key: &str) -> &'a str {
    b.get(key).and_then(Value::as_str).unwrap_or("")
}

async fn setup_needed(State(s): State<AppState>) -> AppResult<Json<Value>> {
    // the first admin is made on the server itself: `florida-3pl create-admin` (see README)
    Ok(Json(json!({ "setup": auth::user_count(&s.db).await? == 0, "allowed": false })))
}

async fn login(State(s): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(b): Json<Value>) -> AppResult<Response> {
    let ip = client_ip(&headers, &peer);
    // Two limits: per address (8 misses) and per account name (30 misses from anywhere, so a spread-out guesser is slowed too).
    let uname: String = text(&b, "username").trim().to_lowercase().chars().filter(|c| !c.is_control()).take(40).collect();
    let user_key = format!("user:{uname}");
    {
        let mut failed = s.failed.lock().unwrap();
        failed.retain(|_, v| {
            v.retain(|t| t.elapsed() < FAIL_WINDOW);
            !v.is_empty()
        });
        let over_ip = failed.get(&ip).is_some_and(|l| l.len() >= MAX_FAILS);
        let over_user = failed.get(&user_key).is_some_and(|l| l.len() >= MAX_FAILS_PER_USER);
        if over_ip || over_user {
            tracing::warn!(target: "security", "sign-in blocked (too many misses) ip={ip} user={uname}");
            return Err(AppError::TooMany("Too many failed sign-ins. Wait 10 minutes and try again.".into()));
        }
    }
    let Some(user) = auth::check_login(&s.db, text(&b, "username"), text(&b, "password")).await? else {
        tracing::warn!(target: "security", "failed sign-in ip={ip} user={uname}");
        let mut failed = s.failed.lock().unwrap();
        failed.entry(ip).or_default().push(Instant::now());
        failed.entry(user_key).or_default().push(Instant::now());
        return Err(AppError::Unauthorized_with("Wrong username or password."));
    };
    s.failed.lock().unwrap().remove(&ip);
    let token = auth::new_session(&s.db, &user.username).await?;
    Ok((AppendHeaders([(SET_COOKIE, auth::cookie_value(&token, is_https(&headers), None))]), Json(json!({ "ok": true }))).into_response())
}

async fn logout(State(s): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if let Some(t) = auth::token_of(&headers) {
        auth::end_session(&s.db, &t).await?;
    }
    Ok((AppendHeaders([(SET_COOKIE, auth::cookie_value("", is_https(&headers), Some(0))), (SET_COOKIE, crate::pma::clear_cookie())]), Json(json!({ "ok": true }))).into_response())
}

async fn me(CurrentUser(u): CurrentUser, State(s): State<AppState>) -> Json<Value> {
    let roles = auth::role_labels(&s.db).await.unwrap_or_else(|_| json!({}));
    Json(json!({ "username": u.username, "name": u.name, "role": u.role, "is_super": u.is_super, "tabs": u.tabs, "roles": roles, "tz_offset_min": clock::offset_minutes() }))
}

async fn change_password(CurrentUser(u): CurrentUser, State(s): State<AppState>, headers: HeaderMap, Json(b): Json<Value>) -> AppResult<Response> {
    if auth::check_login(&s.db, &u.username, text(&b, "current")).await?.is_none() {
        return Err(AppError::bad("Your current password is wrong."));
    }
    auth::set_password(&s.db, &u.username, text(&b, "new")).await?;
    let token = auth::new_session(&s.db, &u.username).await?; // the change signed them out everywhere: keep this browser in
    Ok((AppendHeaders([(SET_COOKIE, auth::cookie_value(&token, is_https(&headers), None))]), Json(json!({ "ok": true }))).into_response())
}

// ---------------------------------------------------------------- team (admins)

async fn team_list(CurrentUser(me): CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    auth::need_tab(&me, "team")?;
    Ok(Json(json!({ "items": auth::list_users(&s.db).await?, "roles": auth::list_roles(&s.db).await? })))
}

/// Admins add, change and remove the roles people can be given.
async fn roles_action(_a: auth::AdminUser, State(s): State<AppState>, Path(action): Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let tabs = tab_list(&b);
    match action.as_str() {
        "add" => auth::add_role(&s.db, text(&b, "label"), tabs.as_deref()).await?,
        "edit" => auth::edit_role(&s.db, text(&b, "name"), text(&b, "label"), tabs.as_deref()).await?,
        "remove" => auth::remove_role(&s.db, text(&b, "name")).await?,
        _ => return Err(AppError::NotFound),
    }
    Ok(Json(json!({ "roles": auth::list_roles(&s.db).await? })))
}

fn tab_list(b: &Value) -> Option<Vec<String>> {
    b.get("tabs")?.as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
}

async fn team_action(CurrentUser(me): CurrentUser, State(s): State<AppState>, Path(action): Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    auth::need_tab(&me, "team")?;
    let who = text(&b, "username");
    // a member who was given the Team tab manages the other members only: no admin is made, changed, reset or removed by them
    if !me.is_admin() {
        let new_role = b.get("role").and_then(Value::as_str).unwrap_or("member");
        let target_admin = action != "add" && auth::role_of(&s.db, who).await? == "admin";
        if target_admin || ((action == "add" || action == "role") && new_role == "admin") {
            return Err(AppError::Forbidden("Only admins can add or change an admin.".into()));
        }
    }
    // the super admin (the first admin account) is changed by nobody else; the other admins are removed or demoted only by the super admin
    if action != "add" && auth::is_super(&s.db, who).await? {
        let own = who == me.username;
        if matches!(action.as_str(), "remove" | "role" | "tabs") || !own {
            return Err(AppError::Forbidden("The super admin account cannot be changed.".into()));
        }
    } else if matches!(action.as_str(), "remove" | "role") && !me.is_super && auth::role_of(&s.db, who).await? == "admin" {
        return Err(AppError::Forbidden("Only the super admin can remove an admin or change an admin's role.".into()));
    }
    match action.as_str() {
        "add" => {
            let role = b.get("role").and_then(Value::as_str).unwrap_or("member");
            let tabs = tab_list(&b);
            auth::add_user(&s.db, who, text(&b, "name"), text(&b, "password"), role, tabs.as_deref()).await?;
        }
        "tabs" => auth::set_tabs(&s.db, who, &tab_list(&b).unwrap_or_default()).await?,
        "reset" => auth::set_password(&s.db, who, text(&b, "password")).await?,
        "remove" => auth::remove_user(&s.db, who, &me.username).await?,
        "role" => auth::set_role(&s.db, who, text(&b, "role")).await?,
        _ => return Err(AppError::NotFound),
    }
    Ok(Json(json!({ "items": auth::list_users(&s.db).await?, "roles": auth::list_roles(&s.db).await? })))
}
