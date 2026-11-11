//! The Work Log chat, the app's branding (name, colour, logo), the Drive file names and the Admin tab.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Json, Response};
use base64::Engine;
use chrono::{Duration, NaiveDate, NaiveDateTime};
use regex::Regex;
use serde_json::{json, Value};
use sqlx::Row;

use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::{clock, drive, settings, AppState};

// ---------------------------------------------------------------- the Work Log chat

const MAX_LEN: usize = 1000;
const LIMIT: i64 = 1000;

fn msg(id: u64, at: NaiveDateTime, username: String, name: String, text: String) -> Value {
    json!({ "id": id, "at": at.format("%Y-%m-%d %H:%M:%S").to_string(), "username": username, "name": name, "text": text })
}

type MsgRow = (u64, NaiveDateTime, String, String, String);

/// The bubble: this person's own messages of today.
pub async fn chat(CurrentUser(u): CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let today = clock::now().format("%Y-%m-%d").to_string();
    let rows: Vec<MsgRow> = sqlx::query_as("SELECT id, at, username, name, `text` FROM work_log_messages WHERE username = ? AND DATE(at) = ? ORDER BY id LIMIT 300").bind(&u.username).bind(&today).fetch_all(&s.db).await?;
    Ok(Json(json!({ "today": today, "items": rows.into_iter().map(|(a, b, c, d, e)| msg(a, b, c, d, e)).collect::<Vec<_>>() })))
}

pub async fn chat_send(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let text = b.get("text").and_then(Value::as_str).unwrap_or("").replace("\r\n", "\n").trim().to_string();
    if text.is_empty() {
        return Err(AppError::bad("Type a message first."));
    }
    if text.chars().count() > MAX_LEN {
        return Err(AppError::bad(format!("A message can be up to {MAX_LEN} characters.")));
    }
    // a gentle brake against a stuck key or a script: 30 messages a minute
    let since = clock::now() - Duration::seconds(60);
    let recent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_log_messages WHERE username = ? AND at >= ?").bind(&u.username).bind(since).fetch_one(&s.db).await?;
    if recent >= 30 {
        return Err(AppError::bad("Too many messages. Wait a moment."));
    }
    let at = clock::now();
    let id = sqlx::query("INSERT INTO work_log_messages (at, username, name, `text`) VALUES (?,?,?,?)").bind(at).bind(&u.username).bind(&u.name).bind(&text).execute(&s.db).await?.last_insert_id();
    s.bump();
    Ok(Json(json!({ "item": msg(id, at, u.username, u.name, text) })))
}

/// Admins: everybody's messages, newest first, filtered by date, person and words.
pub async fn worklog(CurrentUser(_a): CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    crate::auth::need_tab(&_a, "worklog")?;
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let (mut where_, mut args): (Vec<String>, Vec<String>) = (vec![], vec![]);
    if !get("date").is_empty() {
        if NaiveDate::parse_from_str(get("date"), "%Y-%m-%d").is_err() || get("date").len() != 10 {
            return Err(AppError::bad("Pick a date."));
        }
        where_.push("DATE(at) = ?".into());
        args.push(get("date").into());
    }
    if !get("user").is_empty() {
        where_.push("username = ?".into());
        args.push(get("user").into());
    }
    for w in get("q").to_lowercase().split_whitespace().take(8) {
        where_.push("(LOWER(`text`) LIKE ? OR LOWER(name) LIKE ?)".into());
        let like = format!("%{}%", w.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        args.extend([like.clone(), like]);
    }
    let clause = if where_.is_empty() { String::new() } else { format!(" WHERE {}", where_.join(" AND ")) };
    let count_sql = format!("SELECT COUNT(*) FROM work_log_messages{clause}");
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql);
    for a in &args {
        cq = cq.bind(a.clone());
    }
    let total = cq.fetch_one(&s.db).await?;
    let rows_sql = format!("SELECT id, at, username, name, `text` FROM work_log_messages{clause} ORDER BY id DESC LIMIT {LIMIT}");
    let mut rq = sqlx::query_as::<_, MsgRow>(&rows_sql);
    for a in &args {
        rq = rq.bind(a.clone());
    }
    let rows = rq.fetch_all(&s.db).await?;
    let people: Vec<(String, String)> = sqlx::query_as("SELECT CAST(username AS CHAR), CAST(MAX(name) AS CHAR) FROM work_log_messages GROUP BY username ORDER BY MAX(name)").fetch_all(&s.db).await?;
    Ok(Json(json!({
        "items": rows.into_iter().map(|(a, b, c, d, e)| msg(a, b, c, d, e)).collect::<Vec<_>>(), "total": total,
        "people": people.into_iter().map(|(u, n)| json!({ "username": u, "name": n })).collect::<Vec<_>>(), "today": clock::now().format("%Y-%m-%d").to_string(),
    })))
}

// ---------------------------------------------------------------- branding

pub async fn branding_save(CurrentUser(_a): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    crate::auth::need_tab(&_a, "team")?;
    let mut cur: Value = serde_json::from_str(crate::routes::DEFAULT_BRANDING)?;
    if let Some(Value::Object(m)) = settings::get(&s.db, "branding").await? {
        for (k, v) in m {
            cur[k] = v;
        }
    }
    let name: String = b.get("name").and_then(Value::as_str).unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ").chars().take(40).collect();
    let initials: String = b.get("initials").and_then(Value::as_str).unwrap_or("").split_whitespace().collect::<String>().chars().take(3).collect();
    let color = b.get("color").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
    if name.is_empty() {
        return Err(AppError::bad("Give the app a name."));
    }
    if color.len() != 7 || !color.starts_with('#') || !color[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::bad("Pick a colour."));
    }
    let mut new = json!({ "name": name, "initials": if initials.is_empty() { name.chars().take(2).collect::<String>().to_uppercase() } else { initials }, "color": color, "logo": cur["logo"], "v": cur["v"] });
    let v = new["v"].as_i64().unwrap_or(0);
    if b.get("remove_logo").and_then(Value::as_bool).unwrap_or(false) {
        new["logo"] = json!("");
        new["v"] = json!(v + 1);
        sqlx::query("DELETE FROM app_settings WHERE k = 'branding_logo'").execute(&s.db).await?;
    } else if let Some(logo) = b.get("logo").and_then(Value::as_str).filter(|l| !l.is_empty()) {
        let raw = base64::engine::general_purpose::STANDARD.decode(logo.rsplit(',').next().unwrap_or("").trim()).map_err(|_| AppError::bad("That image could not be read."))?;
        let kind = if raw.starts_with(b"\x89PNG\r\n\x1a\n") { "image/png" } else if raw.starts_with(b"\xff\xd8\xff") { "image/jpeg" } else if raw.starts_with(b"GIF8") { "image/gif" } else if raw.len() > 12 && &raw[..4] == b"RIFF" && &raw[8..12] == b"WEBP" { "image/webp" } else { "" };
        if kind.is_empty() {
            return Err(AppError::bad("Use a PNG, JPG, GIF or WebP image."));
        }
        if raw.len() > 1_500_000 {
            return Err(AppError::bad("The logo can be up to 1.5 MB."));
        }
        settings::put(&s.db, "branding_logo", &json!(base64::engine::general_purpose::STANDARD.encode(&raw))).await?;
        new["logo"] = json!(kind);
        new["v"] = json!(v + 1);
    }
    settings::put(&s.db, "branding", &new).await?;
    s.bump();
    Ok(Json(new))
}

pub async fn branding_logo(State(s): State<AppState>) -> AppResult<Response> {
    let b = settings::get(&s.db, "branding").await?.unwrap_or(json!({}));
    let kind = b["logo"].as_str().unwrap_or("").to_string();
    let data = settings::get(&s.db, "branding_logo").await?.and_then(|v| v.as_str().map(str::to_string));
    match (kind.is_empty(), data) {
        (false, Some(d)) => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(d).unwrap_or_default();
            Ok(([(CONTENT_TYPE, kind), (CACHE_CONTROL, "no-cache".to_string())], bytes).into_response())
        }
        _ => Err(AppError::NotFound),
    }
}

// ---------------------------------------------------------------- Drive file names

pub async fn drive_names(_u: CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let ids: Vec<String> = b.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    Ok(Json(drive::names(&s.drive, &s.db, ids).await?))
}

// ---------------------------------------------------------------- the Admin tab

fn plain(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// A label file is named after the customer plus the tracking number (Heather-Taglieri---9400136106196324808946.pdf) or the label
/// number (LN-296Amy Wilson.pdf). It fits the order when the name holds the customer AND one of the two numbers; a number must stand
/// alone (LN-29 is not in LN-296). Returns "" when it fits, otherwise what is wrong.
pub fn label_fits(file_name: &str, customer: &str, label: &str, tracking: &str) -> &'static str {
    // built once: this runs for every label link in the sheet (tens of thousands of times)
    static EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(pdf|png|jpe?g)\s*$").unwrap());
    static COPY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d+\)\s*$").unwrap()); // a copy that Drive named "... (1).pdf" is the same file name
    let stem = EXT.replace(file_name, "").to_string();
    let f = plain(&COPY.replace(&stem, ""));
    let who = plain(customer);
    let has = |n: &str| {
        if n.is_empty() {
            return false;
        }
        let mut from = 0;
        while let Some(p) = f[from..].find(n) {
            let (a, b) = (from + p, from + p + n.len());
            let before_digit = f[..a].chars().next_back().is_some_and(|c| c.is_ascii_digit());
            let after_digit = f[b..].chars().next().is_some_and(|c| c.is_ascii_digit());
            if !before_digit && !after_digit {
                return true;
            }
            from = a + 1;
        }
        false
    };
    let number_ok = has(&plain(label)) || has(&plain(tracking));
    // Strict on purpose: a name the label maker cut short (...ATTN-Savannah for "... ATTN Savannah Sabanosh") is flagged, and a person
    // decides with the Approve button on the Admin tab.
    let name_ok = who.is_empty() || f.contains(&who);
    match (name_ok, number_ok) {
        (true, true) => "",
        (false, false) => "Different customer and number",
        (false, true) => "Different customer name",
        (true, false) => "No matching label or tracking number",
    }
}

/// Check every Shipping Label Link against its order by the file's name. The names come from Google a few at a time in the background
/// (not shipped orders first, newest first), so `checked` grows until it reaches `total`.
async fn label_audit(s: &AppState) -> AppResult<Value> {
    // The result only changes when the data changes or more file names arrive from Google, so it is kept (at most a minute) and
    // reused by every tab and every person. What Google is doing right now (`reading`) is always read fresh.
    let key = (s.version_now(), sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM drive_names").fetch_one(&s.db).await?);
    if let Some((k, at, v)) = AUDIT_CACHE.lock().unwrap().as_ref() {
        if *k == key && at.elapsed().as_secs() < 60 {
            let mut v = v.clone();
            v["reading"] = s.drive.state();
            return Ok(v);
        }
    }
    let fresh = label_audit_uncached(s).await?;
    *AUDIT_CACHE.lock().unwrap() = Some((key, std::time::Instant::now(), fresh.clone()));
    Ok(fresh)
}

static AUDIT_CACHE: LazyLock<Mutex<Option<((u64, i64), std::time::Instant, Value)>>> = LazyLock::new(|| Mutex::new(None));
static COUNTS_CACHE: LazyLock<Mutex<Option<(u64, std::time::Instant, Vec<i64>)>>> = LazyLock::new(|| Mutex::new(None));

async fn label_audit_uncached(s: &AppState) -> AppResult<Value> {
    let sch = s.schema();
    let c = |h: &str| sch.c(h);
    let sql = format!(
        "SELECT rn, {}, {}, {}, {} FROM orders_text WHERE trim({}) <> '' AND trim({}) <> '' ORDER BY (upper(trim({})) = 'TRUE'), rn DESC",
        c("Label Number"), c("Customer Name"), c("Tracking Number To Customer"), c("Shipping Label Link"), c("Store Order No."), c("Shipping Label Link"), c("Shipped to Customer")
    );
    let rows = sqlx::query(&sql).fetch_all(&s.db).await?;
    let known = drive::all_names(&s.db).await?;
    let id_re = Regex::new(r"(?:/d/|[?&]id=)([-\w]{25,})").unwrap();
    let now = clock::now();
    // links a person already looked at and approved: (row, file) -> who and when
    let approvals: HashMap<(u64, String), (String, String)> = sqlx::query("SELECT rn, CAST(file_id AS CHAR), CAST(approved_by AS CHAR), approved_at FROM label_approvals")
        .fetch_all(&s.db)
        .await?
        .iter()
        .map(|r| ((r.try_get::<u64, _>(0).unwrap_or(0), r.try_get_unchecked::<String, _>(1).unwrap_or_default()), (r.try_get_unchecked::<String, _>(2).unwrap_or_default(), r.try_get::<NaiveDateTime, _>(3).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default())))
        .collect();
    let (mut todo, mut mismatches, mut approved, mut checked, mut unread, mut with_id) = (vec![], vec![], vec![], 0, 0, 0);
    for r in &rows {
        let t = |i: usize| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
        let (label, customer, tracking, link) = (t(1), t(2), t(3), t(4));
        let Some(m) = id_re.captures(&link) else { continue };
        let fid = m[1].to_string();
        with_id += 1;
        match known.get(&fid) {
            None => todo.push(fid),
            Some((name, at)) if name.is_empty() => {
                unread += 1; // private, deleted or not readable: cannot be judged
                if (now - *at).num_seconds() >= drive::RETRY {
                    todo.push(fid);
                }
            }
            Some((name, _)) => {
                checked += 1;
                let why = label_fits(name, &customer, &label, &tracking);
                if !why.is_empty() {
                    let rn = r.try_get::<u64, _>(0).unwrap_or(0);
                    let item = json!({ "rn": rn, "file_id": fid, "label": label, "customer": customer, "tracking": tracking, "file": name, "why": why });
                    match approvals.get(&(rn, fid.clone())) {
                        Some((by, at)) => approved.push(json!({ "rn": rn, "file_id": fid, "label": label, "customer": customer, "tracking": tracking, "file": name, "why": why, "by": by, "at": at })),
                        None => mismatches.push(item),
                    }
                }
            }
        }
    }
    if !todo.is_empty() {
        drive::crawl(&s.drive, &s.db, todo);
    }
    Ok(json!({ "checked": checked, "unread": unread, "total": with_id, "mismatches": mismatches, "approved": approved, "reading": s.drive.state() }))
}

pub async fn admin_labels(CurrentUser(_a): CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    crate::auth::need_tab(&_a, "admin")?;
    Ok(Json(label_audit(&s).await?))
}

/// An admin looked at a flagged label link and approves it (`approve` true) or takes the approval back (false). It is tied to the order row
/// and the Drive file, so a link changed to another file is checked again.
pub async fn label_approve(CurrentUser(me): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    crate::auth::need_tab(&me, "admin")?;
    let rn = b.get("rn").and_then(Value::as_u64).ok_or_else(|| AppError::bad("Which order?"))?;
    let file_id = b.get("file_id").and_then(Value::as_str).unwrap_or("");
    if file_id.len() < 10 || file_id.len() > 100 || !file_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(AppError::bad("Which label file?"));
    }
    if b.get("approve").and_then(Value::as_bool).unwrap_or(true) {
        if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM orders WHERE rn = ?").bind(rn).fetch_one(&s.db).await? == 0 {
            return Err(AppError::bad("That order is no longer there."));
        }
        sqlx::query("REPLACE INTO label_approvals (rn, file_id, approved_by, approved_at) VALUES (?,?,?,?)").bind(rn).bind(file_id).bind(&me.name).bind(clock::now()).execute(&s.db).await?;
    } else {
        sqlx::query("DELETE FROM label_approvals WHERE rn = ? AND file_id = ?").bind(rn).bind(file_id).execute(&s.db).await?;
    }
    s.bump(); // the Admin cards and the list look again
    Ok(Json(json!({ "ok": true })))
}

async fn n(s: &AppState, sql: &str) -> AppResult<i64> {
    Ok(sqlx::query(sql).fetch_one(&s.db).await?.try_get_unchecked::<i64, _>(0).unwrap_or(0))
}

/// The Admin tab's cards: things to fix in All Orders. Labels made ahead of time have no order yet, so they are not counted.
pub async fn admin_cards(CurrentUser(_a): CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    crate::auth::need_tab(&_a, "admin")?;
    let sch = s.schema();
    let c = |h: &str| sch.c(h);
    let enc = |h: &str| h.replace(' ', "%20");
    let mut out = vec![];
    // The five simple counts in ONE pass over the orders (they used to be five passes), and kept until the data changes (at most a
    // minute), so opening the tab in several places does not repeat the work.
    // order: no image, no brand, to void, no label link, return not refunded, then the two duplicate counts
    let cached = COUNTS_CACHE.lock().unwrap().as_ref().filter(|(v, at, _)| *v == s.version_now() && at.elapsed().as_secs() < 60).map(|(_, _, c)| c.clone());
    let counts = match cached {
        Some(c) => c,
        None => {
            let has = format!("trim({}) <> ''", c("Store Order No."));
            // the Missing Image / Missing Brand cards leave out the orders that will never ship: Cancelled and Unfulfilled
            // the Label Link Empty card also leaves out the orders that will not ship (Cancelled, Refunded, For Refund, Unfulfilled) and the ones taken from stock
            let shipping = format!("lower(trim({})) NOT IN ('cancelled', 'refunded', 'for refund', 'unfulfilled') AND upper(trim({})) <> 'TRUE'", c("Source Order Status"), c("Stock"));
            let live = format!("lower(trim({})) NOT IN ('cancelled', 'unfulfilled')", c("Source Order Status"));
            let sql = format!(
                "SELECT CAST(COALESCE(SUM({has} AND {live} AND trim({img}) = ''), 0) AS SIGNED), CAST(COALESCE(SUM({has} AND {live} AND trim({brand}) = ''), 0) AS SIGNED), \
                 CAST(COALESCE(SUM(upper(trim({tv})) = 'TRUE' AND upper(trim({lv})) <> 'TRUE'), 0) AS SIGNED), CAST(COALESCE(SUM({has} AND {shipping} AND trim({link}) = ''), 0) AS SIGNED), \
                 CAST(COALESCE(SUM(upper(trim({sa})) = 'TRUE' AND lower(trim({st})) <> 'refunded'), 0) AS SIGNED) FROM orders_text",
                img = c("Product Image Link"), brand = c("Brand"), tv = c("To Void"), lv = c("Label Voided"), link = c("Shipping Label Link"), sa = c("Shipped to Amazon"), st = c("Source Order Status")
            );
            let row = sqlx::query(&sql).fetch_one(&s.db).await?;
            let mut v: Vec<i64> = (0..5).map(|i| row.try_get_unchecked::<i64, _>(i).unwrap_or(0)).collect();
            let dup = |col: String| format!("SELECT CAST(COALESCE(SUM(n), 0) AS SIGNED) FROM (SELECT COUNT(*) n FROM orders_text WHERE trim({col}) <> '' GROUP BY lower(trim({col})) HAVING COUNT(*) > 1) d");
            v.push(n(&s, &dup(c("Tracking Number To Customer"))).await?);
            v.push(n(&s, &dup(c("Shipping Label Link"))).await?);
            *COUNTS_CACHE.lock().unwrap() = Some((s.version_now(), std::time::Instant::now(), v.clone()));
            v
        }
    };
    for (i, (key, label)) in [("Product Image Link", "Missing Product Image Link"), ("Brand", "Missing Brand")].into_iter().enumerate() {
        out.push(json!({ "key": key, "label": label, "n": counts[i], "note": "orders with an empty cell, leaving out Cancelled and Unfulfilled", "link": format!("#orders?blank={}&notstatus=Cancelled,Unfulfilled", enc(key)) }));
    }
    out.push(json!({ "key": "tovoid", "label": "To Void, Label Not Yet Voided", "n": counts[2], "note": "To Void is checked, Label Voided is not", "link": "#orders?show=tovoid" }));
    out.push(json!({ "key": "dupusps", "label": "USPS Tracking Duplicate", "n": counts[5], "note": "rows sharing a Tracking Number To Customer", "link": "#orders?show=dupusps" }));
    out.push(json!({ "key": "duplabel", "label": "Label Link Duplicate", "n": counts[6], "note": "rows sharing a Shipping Label Link", "link": "#orders?show=duplabel" }));
    out.push(json!({ "key": "nolabel", "label": "Label Link Empty", "n": counts[3], "note": "orders with no Shipping Label Link, leaving out Cancelled, Refunded, For Refund, Unfulfilled and Stock", "link": "#orders?blank=Shipping%20Label%20Link&notstatus=Cancelled,Refunded,For%20Refund,Unfulfilled&notstock=1" }));
    let pending = counts[4];
    let a = label_audit(&s).await?;
    let rd = &a["reading"];
    let mut note = format!("checked {} of {} links", fmt(a["checked"].as_i64().unwrap_or(0)), fmt(a["total"].as_i64().unwrap_or(0)));
    if a["unread"].as_i64().unwrap_or(0) > 0 {
        note += &format!(" · {} private or deleted", fmt(a["unread"].as_i64().unwrap_or(0)));
    }
    if rd["limited"].as_bool().unwrap_or(false) {
        note += " · Google is limiting reads, retrying soon";
    } else if rd["queued"].as_i64().unwrap_or(0) > 0 {
        note += " · reading the rest…";
    }
    out.push(json!({ "key": "labelmismatch", "label": "Label Link Mismatch", "n": a["mismatches"].as_array().map_or(0, Vec::len), "note": note, "panel": "labels" }));
    out.push(json!({ "key": "returnpending", "label": "Processed Return, Not Yet Refunded", "n": pending, "note": "Shipped to Amazon is checked, status is not Refunded", "link": "#orders?show=returnpending" }));
    Ok(Json(json!({ "cards": out })))
}

fn fmt(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// The Admin tab's "3PL Daily Update" for one day, the way the 3PL admin reports it:
///  - arrived: the packages that arrived that day (Delivered rows with that Delivery Date, the same as the Dashboard's "Packages Arrived Today")
///  - shipped: the packages shipped to the customer ON that day, split by their Delivery Date
///  - pending: the Delivered packages with that Delivery Date or an earlier one that were still not shipped at the end of that day, split by Delivery Date
/// Shipped means "Shipped to Customer" is ticked or the ship date is filled (a ship date after the day does not count as shipped yet).
pub async fn admin_daily(CurrentUser(_a): CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    crate::auth::need_tab(&_a, "admin")?;
    let day = q.get("date").cloned().unwrap_or_default();
    if day.len() != 10 || NaiveDate::parse_from_str(&day, "%Y-%m-%d").is_err() {
        return Err(AppError::bad("Pick a date."));
    }
    let sch = s.schema();
    let c = |h: &str| sch.c(h);
    let (dt, st, sc, sb) = (c("Delivery Date"), c("Source Order Status"), c("SHIPPED TO  CUSTOMER DATE"), c("Shipped to Customer"));
    // what is still waiting in the warehouse (the Dashboard's "Total Packages in Warehouse" rule): not returned to Amazon, not from stock,
    // not marked not received, and the tracking status is still Preshipment (a return to Amazon / sender / store is not a package to ship)
    let waiting = format!("{} <> 'TRUE' AND {} <> 'TRUE' AND {} <> 'TRUE' AND {} = 'Preshipment'", c("Shipped to Amazon"), c("Stock"), c("Package Not Received"), c("Tracking Status"));
    let by_date = |sql: String, args: Vec<String>| {
        let s = s.clone();
        async move {
            let mut qy = sqlx::query(&sql);
            for a in &args {
                qy = qy.bind(a);
            }
            let rows = qy.fetch_all(&s.db).await?;
            Ok::<_, AppError>(rows.iter().map(|r| json!({ "date": r.try_get_unchecked::<String, _>(0).unwrap_or_default(), "n": r.try_get_unchecked::<i64, _>(1).unwrap_or(0) })).collect::<Vec<_>>())
        }
    };
    let arrived: i64 = sqlx::query(&format!("SELECT COUNT(*) FROM orders_text WHERE {st} = 'Delivered' AND substr({dt}, 1, 10) = ?"))
        .bind(&day)
        .fetch_one(&s.db)
        .await?
        .try_get_unchecked::<i64, _>(0)
        .unwrap_or(0);
    let shipped = by_date(format!("SELECT substr({dt}, 1, 10), COUNT(*) FROM orders_text WHERE substr({sc}, 1, 10) = ? GROUP BY 1 ORDER BY 1"), vec![day.clone()]).await?;
    let pending = by_date(
        format!(
            "SELECT substr({dt}, 1, 10), COUNT(*) FROM orders_text WHERE {st} = 'Delivered' AND substr({dt}, 1, 10) <> '' AND substr({dt}, 1, 10) <= ? AND {waiting} \
             AND NOT ((upper(trim({sb})) = 'TRUE' OR trim({sc}) <> '') AND (trim({sc}) = '' OR substr({sc}, 1, 10) <= ?)) GROUP BY 1 ORDER BY 1"
        ),
        vec![day.clone(), day.clone()],
    )
    .await?;
    Ok(Json(json!({ "date": day, "arrived": arrived, "shipped": shipped, "pending": pending })))
}

#[cfg(test)]
mod tests {
    use super::label_fits;

    #[test]
    fn a_name_cut_short_by_the_label_maker_is_flagged_for_a_person_to_approve() {
        // "Sabanosh", the last letter of "pCard" and the first letter of "Emily" are missing from the file names
        assert_eq!(label_fits("NSA-Mid-South-MWR-Com-Rec-ATTN-Savannah---9300111043900406218871.pdf", "NSA Mid-South MWR Com Rec ATTN Savannah Sabanosh", "LN-49047", "9300111043900406218871"), "Different customer name");
        assert_eq!(label_fits("Denver-Health-Attn-Delane-Gift-Shop-pCar---9334611043900226863048.pdf", "Denver Health Attn Delane Gift Shop pCard", "LN-48981", "9334611043900226863048"), "Different customer name");
        assert_eq!(label_fits("mily-Moore---9300111043900414089531.pdf", "Emily Moore", "LN-7", "9300111043900414089531"), "Different customer name");
    }

    #[test]
    fn the_normal_cases_are_unchanged() {
        assert_eq!(label_fits("Heather-Taglieri---9400136106196324808946.pdf", "Heather Taglieri", "LN-231", "9400136106196324808946"), "");
        assert_eq!(label_fits("LN-296Amy Wilson.pdf", "Amy Wilson", "LN-296", ""), "");
        assert_eq!(label_fits("Heather-Taglieri---9400136106196324808946.pdf", "Amy Wilson", "LN-231", "9400136106196324808946"), "Different customer name");
        assert_eq!(label_fits("Heather-Taglieri---9400136106196324808946.pdf", "Heather Taglieri", "LN-231", "9999999999999999999999"), "No matching label or tracking number");
        assert_eq!(label_fits("Heather-Taglieri---9400136106196324808946.pdf", "Amy Wilson", "LN-5", "123"), "Different customer and number");
    }

    #[test]
    fn a_different_person_is_flagged() {
        assert_eq!(label_fits("NSA-Mid-South-MWR-Com-Rec-ATTN-Jordan---9300111043900406218871.pdf", "NSA Mid-South MWR Com Rec ATTN Savannah Sabanosh", "LN-1", "9300111043900406218871"), "Different customer name");
        assert_eq!(label_fits("Amy---9300111043900406218871.pdf", "Amy Wilson", "LN-1", "9300111043900406218871"), "Different customer name");
    }
}
