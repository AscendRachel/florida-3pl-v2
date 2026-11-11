//! Notifications: somebody wrote a note on an order.
//!
//! Every time a person saves a note (the Notes cell of an order, from All Orders or from an Order Cards edit window), one event is
//! recorded with who wrote it, which order (row number and label) and the note itself. Everybody else who can open All Orders or
//! Order Cards sees it in the bell at the top: the writer never gets their own. Each person has a "seen up to" mark, so the bell counts
//! only what is new for them. When the cell already held a note written by somebody else, the event also records whose note it answers.

use axum::extract::State;
use axum::response::Json;
use chrono::{Duration, NaiveDateTime};
use serde_json::{json, Value};
use sqlx::Row;

use crate::auth::{CurrentUser, User};
use crate::error::AppResult;
use crate::{clock, AppState};

const KEEP_DAYS: i64 = 60;
const SHOW: i64 = 50;

pub struct NoteEvent {
    pub rn: u64,
    pub label: String,
    pub customer: String,
    pub field: String,
    pub text: String,
    /// What the cell held before.
    pub old: String,
}

pub async fn add(s: &AppState, by: &User, notes: &[NoteEvent]) -> AppResult<()> {
    let now = clock::now();
    for n in notes.iter().take(100) {
        let (mut kind, mut to_user, mut to_name) = ("", String::new(), String::new());
        let prev: Option<(String, String)> = sqlx::query_as("SELECT by_user, by_name FROM notifications WHERE rn = ? AND field = ? ORDER BY id DESC LIMIT 1").bind(n.rn).bind(&n.field).fetch_optional(&s.db).await?;
        if let Some((u, name)) = prev {
            if !n.old.trim().is_empty() && u != by.username {
                kind = if n.text.contains(n.old.trim()) { "reply" } else { "rewrite" };
                (to_user, to_name) = (u, name);
            }
        }
        let text: String = n.text.chars().take(600).collect();
        sqlx::query("INSERT INTO notifications (at, by_user, by_name, by_role, rn, label, customer, field, text, kind, to_user, to_name) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(now).bind(&by.username).bind(&by.name).bind(&by.role).bind(n.rn).bind(&n.label).bind(&n.customer).bind(&n.field).bind(text).bind(kind).bind(to_user).bind(to_name)
            .execute(&s.db)
            .await?;
    }
    sqlx::query("DELETE FROM notifications WHERE at < ?").bind(now - Duration::days(KEEP_DAYS)).execute(&s.db).await?;
    Ok(())
}

async fn top_id(s: &AppState) -> AppResult<u64> {
    Ok(sqlx::query_scalar::<_, Option<u64>>("SELECT MAX(id) FROM notifications").fetch_one(&s.db).await?.unwrap_or(0))
}

/// The newest notes written by other people, with which ones are new for this person.
pub async fn feed(CurrentUser(u): CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let can_orders = u.is_admin() || u.tabs.as_ref().is_none_or(|t| t.iter().any(|x| x == "orders"));
    let can_cards = u.tabs.as_ref().is_none_or(|t| t.iter().any(|x| x == "cards"));
    if !can_orders && !can_cards && !u.is_admin() {
        return Ok(Json(json!({ "items": [], "unread": 0, "can_orders": false })));
    }
    let last: u64 = match sqlx::query_scalar::<_, u64>("SELECT last_id FROM notification_seen WHERE username = ?").bind(&u.username).fetch_optional(&s.db).await? {
        Some(l) => l,
        None => {
            // a new account starts with a clean bell
            let top = top_id(&s).await?;
            sqlx::query("INSERT INTO notification_seen (username, last_id) VALUES (?, ?)").bind(&u.username).bind(top).execute(&s.db).await?;
            top
        }
    };
    let rows = sqlx::query(
        "SELECT id, at, by_user, by_name, by_role, rn, label, customer, field, `text`, CAST(kind AS CHAR), to_user, to_name FROM notifications WHERE by_user <> ? ORDER BY id DESC LIMIT ?",
    )
    .bind(&u.username)
    .bind(SHOW)
    .fetch_all(&s.db)
    .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let t = |i: usize| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
            let id: u64 = r.try_get(0).unwrap_or(0);
            let at: NaiveDateTime = r.try_get(1).unwrap_or_default();
            let to_user = t(11);
            json!({
                "id": id, "at": at.format("%Y-%m-%d %H:%M:%S").to_string(), "by_user": t(2), "by_name": t(3), "by_role": t(4), "rn": r.try_get::<u64, _>(5).unwrap_or(0),
                "label": t(6), "customer": t(7), "field": t(8), "text": t(9), "kind": t(10), "to_user": to_user, "to_name": t(12),
                "unread": id > last, "mine": !to_user.is_empty() && to_user == u.username,
            })
        })
        .collect();
    let unread: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE by_user <> ? AND id > ?").bind(&u.username).bind(last).fetch_one(&s.db).await?;
    Ok(Json(json!({ "items": items, "unread": unread, "can_orders": can_orders && !(u.role == "warehouse" && can_cards) })))
}

pub async fn seen(CurrentUser(u): CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let top = top_id(&s).await?;
    sqlx::query("REPLACE INTO notification_seen (username, last_id) VALUES (?, ?)").bind(&u.username).bind(top).execute(&s.db).await?;
    Ok(Json(json!({ "ok": true })))
}
