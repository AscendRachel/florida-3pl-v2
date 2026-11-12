//! Accounts and sign-in sessions.
//!
//! Passwords are PBKDF2-SHA256 hashes (200,000 rounds, a random salt per person), the same as the Python app, so everybody
//! signs in with the password they already have. A signed-in browser holds a random session token in an HttpOnly cookie;
//! the token is looked up here on every request.

use axum::async_trait;
use axum::extract::FromRequestParts;
use axum::http::header::COOKIE;
use axum::http::request::Parts;
use chrono::{Duration, NaiveDateTime};
use rand::RngCore;
use serde::Serialize;
use sha2::Sha256;
use sqlx::MySqlPool;
use subtle::ConstantTimeEq;

use crate::error::{AppError, AppResult};
use crate::{clock, AppState};

pub const COOKIE_NAME: &str = "f3pl_session";
pub const SESSION_DAYS: i64 = 14;
pub const ROLES: [&str; 3] = ["admin", "warehouse", "member"];
/// The tabs an admin can hand out to a member.
pub const TABS: [&str; 12] = ["dashboard", "scan", "cards", "orders", "returns", "materials", "checklist", "worklog", "amazon", "import", "team", "admin"];
/// The tabs that used to be for admins only: a member sees one only when it is ticked for them (an old "every tab" member does not get them).
pub const ADMIN_TABS: [&str; 5] = ["worklog", "amazon", "import", "team", "admin"];
/// Data that belongs to one tab (any of the tabs listed may read it). Shared data (orders, meta ...) stays open to every signed-in person.
const API_TABS: [(&str, &[&str]); 5] = [
    ("/api/checklist", &["checklist"]),
    ("/api/dashboard", &["dashboard"]),
    ("/api/materials", &["materials", "cards"]),
    ("/api/returns", &["returns", "cards"]),
    ("/api/kohls", &["returns", "cards"]),
];

/// Who is signed in. `tabs` is None for "every tab" (admins always).
#[derive(Clone, Serialize)]
pub struct User {
    pub username: String,
    pub name: String,
    pub role: String,
    pub tabs: Option<Vec<String>>,
    /// The first admin account: nobody else can change or remove it, and only it can remove other admins.
    pub is_super: bool,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    /// Whether this person may use the tab: admins always; a member when it is one of their tabs
    /// (no list = every tab except the ones in ADMIN_TABS).
    pub fn can(&self, tab: &str) -> bool {
        if self.is_admin() {
            return true;
        }
        match &self.tabs {
            Some(t) => t.iter().any(|x| x == tab),
            None => !ADMIN_TABS.contains(&tab),
        }
    }
}

/// Stops the request unless the person may use this tab.
pub fn need_tab(user: &User, tab: &str) -> AppResult<()> {
    if user.can(tab) {
        Ok(())
    } else {
        Err(AppError::Forbidden("This tab is not open to you.".into()))
    }
}

#[derive(sqlx::FromRow)]
struct UserRow {
    username: String,
    name: String,
    pw_hash: String,
    salt: String,
    role: String,
    tabs: Option<String>,
    is_super: bool,
}

impl UserRow {
    fn user(self) -> User {
        let tabs = if self.role == "admin" { None } else { parse_tabs(self.tabs.as_deref()) };
        User { username: self.username, name: self.name, is_super: self.is_super && self.role == "admin", role: self.role, tabs }
    }
}

/// The stored JSON text as a list of tab keys, or None for "every tab".
pub fn parse_tabs(stored: Option<&str>) -> Option<Vec<String>> {
    let list: Vec<String> = serde_json::from_str(stored?).ok()?;
    let got: Vec<String> = TABS.iter().filter(|t| list.iter().any(|x| x == *t)).map(|t| t.to_string()).collect();
    if got.is_empty() {
        None
    } else {
        Some(got)
    }
}

/// What is stored for the tabs picked: an error when none, NULL when exactly the ordinary tabs are picked (none of ADMIN_TABS).
pub fn clean_tabs(tabs: &[String]) -> AppResult<Option<String>> {
    let got: Vec<&str> = TABS.iter().copied().filter(|t| tabs.iter().any(|x| x == t)).collect();
    if got.is_empty() {
        return Err(AppError::bad("Pick at least one tab."));
    }
    let ordinary = TABS.iter().filter(|t| !ADMIN_TABS.contains(t)).count();
    let plain_all = got.len() == ordinary && got.iter().all(|t| !ADMIN_TABS.contains(t));
    Ok(if plain_all { None } else { Some(serde_json::to_string(&got)?) })
}

/// False when the person's tabs do not include the tab that owns this API path.
pub fn tab_allowed(user: &User, path: &str) -> bool {
    let Some(tabs) = &user.tabs else { return true };
    if user.is_admin() {
        return true;
    }
    for (prefix, owners) in API_TABS {
        if path == prefix || path.strip_prefix(prefix).is_some_and(|r| r.starts_with('/')) {
            return owners.iter().any(|o| tabs.iter().any(|t| t == o));
        }
    }
    true
}

pub fn hash_pw(password: &str, salt_hex: &str) -> String {
    let salt = hex::decode(salt_hex).unwrap_or_default();
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, 200_000, &mut out);
    hex::encode(out)
}

fn new_salt() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn check_password_rules(password: &str) -> AppResult<()> {
    if password.chars().count() < 10 {
        return Err(AppError::bad("Password must be at least 10 characters."));
    }
    if password.chars().count() > 200 {
        return Err(AppError::bad("Password can be up to 200 characters."));
    }
    Ok(())
}

pub async fn user_count(db: &MySqlPool) -> AppResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE disabled = 0").fetch_one(db).await?)
}

pub async fn add_user(db: &MySqlPool, username: &str, name: &str, password: &str, role: &str, tabs: Option<&[String]>) -> AppResult<String> {
    let username = username.trim().to_lowercase();
    let tabs_text = match (tabs, role) {
        (Some(t), r) if r != "admin" => clean_tabs(t)?,
        _ => None,
    };
    if !(3..=30).contains(&username.chars().count()) || !username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c)) {
        return Err(AppError::bad("Username: 3 to 30 letters, numbers, dots, dashes or underscores."));
    }
    check_password_rules(password)?;
    if !role_exists(db, role).await? {
        return Err(AppError::bad("That role does not exist."));
    }
    if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE username = ?").bind(&username).fetch_one(db).await? > 0 {
        return Err(AppError::bad("That username is taken."));
    }
    let salt = new_salt();
    let (pw, s) = (password.to_string(), salt.clone());
    let hash = tokio::task::spawn_blocking(move || hash_pw(&pw, &s)).await?;
    let shown: String = name.trim().chars().take(60).collect();
    let first_admin = role == "admin" && sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE is_super = 1 AND disabled = 0").fetch_one(db).await? == 0;
    sqlx::query("INSERT INTO users (username, name, pw_hash, salt, role, tabs, created_at, is_super) VALUES (?,?,?,?,?,?,?,?)")
        .bind(&username)
        .bind(if shown.is_empty() { &username } else { &shown })
        .bind(hash)
        .bind(salt)
        .bind(role)
        .bind(tabs_text)
        .bind(clock::now())
        .bind(first_admin)
        .execute(db)
        .await?;
    Ok(username)
}

pub async fn set_password(db: &MySqlPool, username: &str, password: &str) -> AppResult<()> {
    check_password_rules(password)?;
    let salt = new_salt();
    let (pw, s) = (password.to_string(), salt.clone());
    let hash = tokio::task::spawn_blocking(move || hash_pw(&pw, &s)).await?;
    let n = sqlx::query("UPDATE users SET pw_hash = ?, salt = ? WHERE username = ? AND disabled = 0").bind(hash).bind(salt).bind(username).execute(db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::bad("No such user."));
    }
    sqlx::query("DELETE FROM sessions WHERE username = ?").bind(username).execute(db).await?; // signs them out everywhere
    Ok(())
}

/// The person when the username and password are right.
pub async fn check_login(db: &MySqlPool, username: &str, password: &str) -> AppResult<Option<User>> {
    let row: Option<UserRow> = sqlx::query_as("SELECT username, name, pw_hash, salt, role, CAST(tabs AS CHAR) AS tabs, is_super FROM users WHERE username = ? AND disabled = 0")
        .bind(username.trim().to_lowercase())
        .fetch_optional(db)
        .await?;
    let (pw, salt, stored) = (password.to_string(), row.as_ref().map(|r| r.salt.clone()).unwrap_or_else(|| "00".repeat(16)), row.as_ref().map(|r| r.pw_hash.clone()).unwrap_or_default());
    // the same amount of work whether or not the person exists
    let ok = tokio::task::spawn_blocking(move || hash_pw(&pw, &salt).as_bytes().ct_eq(stored.as_bytes()).into()).await?;
    Ok(if ok { row.map(UserRow::user) } else { None })
}

pub async fn new_session(db: &MySqlPool, username: &str) -> AppResult<String> {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    let token = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, b);
    let now = utc_now();
    sqlx::query("DELETE FROM sessions WHERE expires_at < ?").bind(now).execute(db).await?;
    sqlx::query("INSERT INTO sessions (token, username, expires_at) VALUES (?,?,?)").bind(&token).bind(username).bind(now + Duration::days(SESSION_DAYS)).execute(db).await?;
    Ok(token)
}

fn utc_now() -> NaiveDateTime {
    chrono::Utc::now().naive_utc()
}

pub async fn session_user(db: &MySqlPool, token: &str) -> AppResult<Option<User>> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT u.username, u.name, u.pw_hash, u.salt, u.role, CAST(u.tabs AS CHAR) AS tabs, u.is_super FROM sessions s JOIN users u ON u.username = s.username \
         WHERE s.token = ? AND s.expires_at > ? AND u.disabled = 0",
    )
    .bind(token)
    .bind(utc_now())
    .fetch_optional(db)
    .await?;
    Ok(row.map(UserRow::user))
}

pub async fn end_session(db: &MySqlPool, token: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE token = ?").bind(token).execute(db).await?;
    Ok(())
}

#[derive(Serialize, sqlx::FromRow)]
struct ListRow {
    username: String,
    name: String,
    role: String,
    created_at: NaiveDateTime,
    tabs: Option<String>,
    is_super: bool,
}

/// The team list as the Team page expects it: [{username, name, role, created_at, tabs}].
pub async fn list_users(db: &MySqlPool) -> AppResult<Vec<serde_json::Value>> {
    let rows: Vec<ListRow> = sqlx::query_as("SELECT username, name, CAST(role AS CHAR) AS role, created_at, CAST(tabs AS CHAR) AS tabs, is_super FROM users WHERE disabled = 0 ORDER BY is_super DESC, role, username").fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let tabs = if r.role == "admin" { None } else { parse_tabs(r.tabs.as_deref()) };
            serde_json::json!({ "username": r.username, "name": r.name, "role": r.role, "created_at": r.created_at.format("%Y-%m-%d %H:%M").to_string(), "tabs": tabs, "is_super": r.is_super && r.role == "admin" })
        })
        .collect())
}

pub async fn set_tabs(db: &MySqlPool, username: &str, tabs: &[String]) -> AppResult<()> {
    let text = clean_tabs(tabs)?;
    if sqlx::query("UPDATE users SET tabs = ? WHERE username = ? AND disabled = 0").bind(text).bind(username).execute(db).await?.rows_affected() == 0 {
        // MariaDB reports 0 rows when the value did not change: check the person exists
        if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE username = ? AND disabled = 0").bind(username).fetch_one(db).await? == 0 {
            return Err(AppError::bad("No such user."));
        }
    }
    Ok(())
}

async fn admin_count(db: &MySqlPool) -> AppResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE disabled = 0 AND role = 'admin'").fetch_one(db).await?)
}

pub async fn is_super(db: &MySqlPool, username: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE username = ? AND disabled = 0 AND is_super = 1 AND role = 'admin'").bind(username).fetch_one(db).await? > 0)
}

pub async fn role_of(db: &MySqlPool, username: &str) -> AppResult<String> {
    sqlx::query_scalar("SELECT role FROM users WHERE username = ? AND disabled = 0").bind(username).fetch_optional(db).await?.ok_or_else(|| AppError::bad("No such user."))
}

pub async fn remove_user(db: &MySqlPool, username: &str, by: &str) -> AppResult<()> {
    if username == by {
        return Err(AppError::bad("You cannot remove your own account."));
    }
    if is_super(db, username).await? {
        return Err(AppError::Forbidden("The super admin account cannot be removed.".into()));
    }
    if role_of(db, username).await? == "admin" && admin_count(db).await? <= 1 {
        return Err(AppError::bad("There must always be at least one admin."));
    }
    sqlx::query("UPDATE users SET disabled = 1 WHERE username = ?").bind(username).execute(db).await?;
    sqlx::query("DELETE FROM sessions WHERE username = ?").bind(username).execute(db).await?;
    Ok(())
}

pub async fn set_role(db: &MySqlPool, username: &str, role: &str) -> AppResult<()> {
    if role != "admin" && is_super(db, username).await? {
        return Err(AppError::Forbidden("The super admin account cannot be changed.".into()));
    }
    if !role_exists(db, role).await? {
        return Err(AppError::bad("That role does not exist."));
    }
    if role_of(db, username).await? == "admin" && role != "admin" && admin_count(db).await? <= 1 {
        return Err(AppError::bad("There must always be at least one admin."));
    }
    sqlx::query("UPDATE users SET role = ? WHERE username = ?").bind(role).bind(username).execute(db).await?;
    Ok(())
}

// ---------------------------------------------------------------- who is asking

/// The token in the request's cookie.
pub fn token_of(headers: &axum::http::HeaderMap) -> Option<String> {
    let header = headers.get(COOKIE)?.to_str().ok()?;
    header.split(';').filter_map(|c| c.trim().strip_prefix(COOKIE_NAME)?.strip_prefix('=')).map(str::to_string).next()
}

pub fn cookie_value(token: &str, https: bool, max_age: Option<i64>) -> String {
    let age = max_age.unwrap_or(SESSION_DAYS * 86400);
    format!("{COOKIE_NAME}={token}; Max-Age={age}; Path=/; HttpOnly; SameSite=Lax{}", if https { "; Secure" } else { "" })
}

/// A handler that takes `CurrentUser` only runs for a signed-in person who may open the tab that owns the address.
pub struct CurrentUser(pub User);

#[async_trait]
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = token_of(&parts.headers).ok_or(AppError::Unauthorized)?;
        let user = session_user(&state.db, &token).await?.ok_or(AppError::Unauthorized)?;
        if !tab_allowed(&user, parts.uri.path()) {
            return Err(AppError::Forbidden("Your account does not have access to this tab.".into()));
        }
        Ok(Self(user))
    }
}

/// Like `CurrentUser`, but only admins get in.
pub struct AdminUser(pub User);

#[async_trait]
impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;
        if !user.is_admin() {
            return Err(AppError::Forbidden("Only admins can do this.".into()));
        }
        Ok(Self(user))
    }
}

// ---------------------------------------------------------------- roles (added and removed by admins)

pub async fn role_exists(db: &MySqlPool, name: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM roles WHERE name = ?").bind(name).fetch_one(db).await? > 0)
}

/// [{name, label, builtin, tabs, people}] in the order they are offered.
pub async fn list_roles(db: &MySqlPool) -> AppResult<Vec<serde_json::Value>> {
    let rows: Vec<(String, String, Option<String>, i8, i64)> = sqlx::query_as(
        "SELECT CAST(r.name AS CHAR), CAST(r.label AS CHAR), CAST(r.tabs AS CHAR), r.builtin, (SELECT COUNT(*) FROM users u WHERE u.role = r.name AND u.disabled = 0) FROM roles r ORDER BY r.pos, r.label",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|(name, label, tabs, builtin, people)| serde_json::json!({ "name": name, "label": label, "builtin": builtin != 0, "tabs": parse_tabs(tabs.as_deref()), "people": people })).collect())
}

/// The tabs picked for a role, as stored (None when none are picked, and for Admin, who always has every tab).
fn role_tabs(name: &str, tabs: Option<&[String]>) -> AppResult<Option<String>> {
    let Some(list) = tabs else { return Ok(None) };
    let got: Vec<&str> = TABS.iter().copied().filter(|t| list.iter().any(|x| x == t)).collect();
    if got.is_empty() || name == "admin" {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(&got)?))
}

fn role_label_ok(label: &str) -> AppResult<String> {
    let label: String = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.is_empty() || label.chars().count() > 40 {
        return Err(AppError::bad("Give the role a name of up to 40 characters."));
    }
    Ok(label)
}

pub async fn add_role(db: &MySqlPool, label: &str, tabs: Option<&[String]>) -> AppResult<()> {
    let label = role_label_ok(label)?;
    let key: String = label.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).take(20).collect();
    if key.is_empty() {
        return Err(AppError::bad("The role name needs at least one letter or number."));
    }
    let taken: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM roles WHERE name = ? OR LOWER(label) = LOWER(?)").bind(&key).bind(&label).fetch_one(db).await?;
    if taken > 0 {
        return Err(AppError::bad("There is already a role with that name."));
    }
    let pos: i64 = sqlx::query_scalar("SELECT CAST(IFNULL(MAX(pos), 0) + 1 AS SIGNED) FROM roles").fetch_one(db).await?;
    sqlx::query("INSERT INTO roles (name, label, tabs, builtin, pos) VALUES (?,?,?,0,?)").bind(&key).bind(&label).bind(role_tabs(&key, tabs)?).bind(pos).execute(db).await?;
    Ok(())
}

pub async fn edit_role(db: &MySqlPool, name: &str, label: &str, tabs: Option<&[String]>) -> AppResult<()> {
    if !role_exists(db, name).await? {
        return Err(AppError::bad("That role does not exist."));
    }
    let label = role_label_ok(label)?;
    let taken: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM roles WHERE LOWER(label) = LOWER(?) AND name <> ?").bind(&label).bind(name).fetch_one(db).await?;
    if taken > 0 {
        return Err(AppError::bad("There is already a role with that name."));
    }
    sqlx::query("UPDATE roles SET label = ?, tabs = ? WHERE name = ?").bind(&label).bind(role_tabs(name, tabs)?).bind(name).execute(db).await?;
    Ok(())
}

pub async fn remove_role(db: &MySqlPool, name: &str) -> AppResult<()> {
    let builtin: Option<i8> = sqlx::query_scalar("SELECT builtin FROM roles WHERE name = ?").bind(name).fetch_optional(db).await?;
    match builtin {
        None => return Err(AppError::bad("That role does not exist.")),
        _ => {}
    }
    if name == "admin" {
        return Err(AppError::bad("The Admin role cannot be removed."));
    }
    let people: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = ? AND disabled = 0").bind(name).fetch_one(db).await?;
    if people > 0 {
        return Err(AppError::bad(format!("{people} {} this role. Change their role first, then remove it.", if people == 1 { "person still has" } else { "people still have" })));
    }
    sqlx::query("DELETE FROM roles WHERE name = ?").bind(name).execute(db).await?;
    Ok(())
}

/// role key -> label, for showing people's roles by name.
pub async fn role_labels(db: &MySqlPool) -> AppResult<serde_json::Value> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT CAST(name AS CHAR), CAST(label AS CHAR) FROM roles").fetch_all(db).await?;
    Ok(serde_json::Value::Object(rows.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect()))
}
