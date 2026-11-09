//! The Team Daily Check List: the daily tasks of the team (each row in the colour of the agent assigned), the ticks of each day,
//! the numbers the app counts by itself (tasks A, B, D, F, G, O and P; the other tasks are typed), and the two free lists beside it.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration as StdDuration, Instant};

use axum::extract::{Query, State};
use axum::response::Json;
use chrono::{Duration, NaiveDate};
use serde_json::{json, Value};
use sqlx::Row;

use crate::auth::{AdminUser, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::{clock, AppState};

/// The tasks whose number the app counts, and how (shown on the page when the number is hovered).
const COUNTED: [(&str, &str); 7] = [
    ("A", "Right now: orders with a Source Order Number and no Source Tracking Number, whose status is not Delivered, Cancelled, Refunded or For Refund."),
    ("B", "On this day: orders whose Source Order Status changed from In Transit to Delivered."),
    ("D", "On this day: orders that were ticked Package Not Received."),
    ("F", "On this day: rows that were red in All Orders and changed to another colour (actioned / resolved)."),
    ("G", "On this day: rows that were magenta in All Orders and changed to another colour (actioned / resolved)."),
    ("O", "On this day: orders whose empty Brand or Product Image Link was filled with data."),
    ("P", "On this day: orders whose Tracking Status changed from In Transit to Delivered."),
];
const MAX_TEXT: usize = 2000;
const MAX_ROWS: i64 = 500; // the Additional Task and SKU columns can run this many rows down

fn counted(code: &str) -> Option<&'static str> {
    COUNTED.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

fn parse_day(v: Option<&str>) -> AppResult<NaiveDate> {
    match v.filter(|d| !d.is_empty()) {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| AppError::bad("Pick a date.")),
        None => Ok(clock::now().date()),
    }
}

type Backlog = HashMap<&'static str, i64>;
static BACKLOG: LazyLock<Mutex<Option<(u64, Instant, Backlog)>>> = LazyLock::new(|| Mutex::new(None));

/// The one number that is "right now": A. Kept for 20 seconds (and until something changes), and written down as the
/// number of today, so a past day still shows what it was the last time somebody looked.
async fn backlog(s: &AppState) -> AppResult<Backlog> {
    let v = s.version_now();
    if let Some((ver, at, b)) = BACKLOG.lock().unwrap().as_ref() {
        if *ver == v && at.elapsed() < StdDuration::from_secs(20) {
            return Ok(b.clone());
        }
    }
    let sch = s.schema();
    let c = |h: &str| sch.c(h);
    let a = sqlx::query(&format!(
        "SELECT COUNT(*) FROM orders_text WHERE trim({}) <> '' AND trim({}) = '' AND lower(trim({})) NOT IN ('delivered', 'cancelled', 'refunded', 'for refund')",
        c("Source Order Number"), c("Source Tracking Number"), c("Source Order Status")
    ))
    .fetch_one(&s.db)
    .await?
    .try_get_unchecked::<i64, _>(0)
    .unwrap_or(0);
    let b: Backlog = HashMap::from([("A", a)]);
    let today = clock::now().date();
    for (k, n) in &b {
        sqlx::query("INSERT INTO checklist_counts (day, task, n) VALUES (?,?,?) ON DUPLICATE KEY UPDATE n = VALUES(n)").bind(today).bind(*k).bind(*n).execute(&s.db).await?;
    }
    *BACKLOG.lock().unwrap() = Some((v, Instant::now(), b.clone()));
    Ok(b)
}

// ---- rows that left red / magenta

/// The cells the status colours of All Orders depend on (the same rules as cformat.rs / cond-format.js; keep them in step).
const COLOUR_FIELDS: [&str; 6] = ["source_order_status", "tracking_status", "package_not_received", "shipped_to_customer", "delivery_date", "source_order_number"];

#[derive(Clone, Default)]
struct RowState {
    status: String,
    tracking: String,
    not_received: String,
    shipped: String,
    delivery: String,
    order_no: String,
}

impl RowState {
    fn set(&mut self, field: &str, v: &str) {
        match field {
            "source_order_status" => self.status = v.to_string(),
            "tracking_status" => self.tracking = v.to_string(),
            "package_not_received" => self.not_received = v.to_string(),
            "shipped_to_customer" => self.shipped = v.to_string(),
            "delivery_date" => self.delivery = v.to_string(),
            "source_order_number" => self.order_no = v.to_string(),
            _ => {}
        }
    }

    /// The colour the row gets, when it is red or magenta ("other" for every other colour or none). The first rule that fits wins, as in the sheet.
    fn colour(&self, today: &str) -> &'static str {
        let is = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b);
        let (st, tr) = (&self.status, &self.tracking);
        if is(st, "refunded") || is(tr, "store returns") || is(tr, "return to sender") || is(&self.not_received, "TRUE") || is(st, "for refund") || is(tr, "return to amazon") {
            return "other";
        }
        if is(&self.shipped, "TRUE") && is(tr, "preshipment") {
            return "magenta";
        }
        if is(tr, "delivered") || is(tr, "in transit") || is(st, "unfulfilled") || is(st, "cancelled") || is(st, "delivered") {
            return "other";
        }
        let d = self.delivery.trim();
        if !self.order_no.trim().is_empty() && (d.is_empty() || d.get(..10).unwrap_or(d) < today) {
            return "red";
        }
        "other"
    }
}

/// (rows that were red and no longer are, rows that were magenta and no longer are) on that day: the row as it was before the first change
/// of that day against the row as it was after the last one. Only the rows with a change in a cell that decides the colour are looked at.
async fn left_colours(s: &AppState, day: NaiveDate) -> AppResult<(i64, i64)> {
    let start = day.and_hms_opt(0, 0, 0).unwrap_or_default();
    let end = start + Duration::days(1);
    let logs: Vec<(u64, String, String, String)> = sqlx::query_as(
        "SELECT rn, field, CAST(IFNULL(old_value, '') AS CHAR), CAST(IFNULL(new_value, '') AS CHAR) FROM edit_log \
         WHERE at >= ? AND at < ? AND field IN ('source_order_status', 'tracking_status', 'package_not_received', 'shipped_to_customer', 'delivery_date', 'source_order_number') ORDER BY id",
    )
    .bind(start)
    .bind(end)
    .fetch_all(&s.db)
    .await?;
    if logs.is_empty() {
        return Ok((0, 0));
    }
    // per row: for each cell the value before the first change and after the last
    let mut per: HashMap<u64, HashMap<&str, (String, String)>> = HashMap::new();
    for (rn, field, old, new) in &logs {
        let Some(f) = COLOUR_FIELDS.iter().find(|f| *f == field) else { continue };
        per.entry(*rn).or_default().entry(*f).and_modify(|e| e.1 = new.clone()).or_insert((old.clone(), new.clone()));
    }
    let sch = s.schema();
    let c = |h: &str| sch.c(h);
    let cols = format!("{}, {}, {}, {}, {}, {}", c("Source Order Status"), c("Tracking Status"), c("Package Not Received"), c("Shipped to Customer"), c("Delivery Date"), c("Source Order Number"));
    let rns: Vec<u64> = per.keys().copied().collect();
    let today = day.format("%Y-%m-%d").to_string();
    let (mut red, mut magenta) = (0, 0);
    for chunk in rns.chunks(400) {
        let sql = format!("SELECT rn, {cols} FROM orders_text WHERE rn IN ({})", vec!["?"; chunk.len()].join(","));
        let mut q = sqlx::query(&sql);
        for rn in chunk {
            q = q.bind(*rn);
        }
        for row in q.fetch_all(&s.db).await? {
            let rn = row.try_get_unchecked::<u64, _>(0).unwrap_or(0);
            let t = |i: usize| row.try_get_unchecked::<String, _>(i).unwrap_or_default();
            let mut now = RowState { status: t(1), tracking: t(2), not_received: t(3), shipped: t(4), delivery: t(5), order_no: t(6) };
            let mut was = now.clone();
            for (field, (old, new)) in per.get(&rn).into_iter().flatten() {
                was.set(field, old);
                now.set(field, new);
            }
            let (before, after) = (was.colour(&today), now.colour(&today));
            if before != after {
                match before {
                    "red" => red += 1,
                    "magenta" => magenta += 1,
                    _ => {}
                }
            }
        }
    }
    Ok((red, magenta))
}

/// Orders whose empty Brand or Product Image Link was filled in on that day.
async fn blanks_filled(s: &AppState, day: NaiveDate) -> AppResult<i64> {
    let start = day.and_hms_opt(0, 0, 0).unwrap_or_default();
    let end = start + Duration::days(1);
    Ok(sqlx::query(
        "SELECT COUNT(DISTINCT rn) FROM edit_log WHERE at >= ? AND at < ? AND field IN ('brand', 'product_image_link') AND IFNULL(old_value, '') = '' AND IFNULL(new_value, '') <> '' AND note <> 'Row added'",
    )
    .bind(start)
    .bind(end)
    .fetch_one(&s.db)
    .await?
    .try_get_unchecked::<i64, _>(0)
    .unwrap_or(0))
}

/// How many orders had this cell change on that day (old value -> new value; `old` None = "was not already TRUE").
async fn changes(s: &AppState, day: NaiveDate, field: &str, old: Option<&str>, new: &str) -> AppResult<i64> {
    let start = day.and_hms_opt(0, 0, 0).unwrap_or_default();
    let end = start + Duration::days(1);
    let q = match old {
        Some(o) => sqlx::query("SELECT COUNT(DISTINCT rn) FROM edit_log WHERE at >= ? AND at < ? AND field = ? AND new_value = ? AND old_value = ?").bind(start).bind(end).bind(field).bind(new).bind(o),
        None => sqlx::query("SELECT COUNT(DISTINCT rn) FROM edit_log WHERE at >= ? AND at < ? AND field = ? AND new_value = ? AND IFNULL(old_value, '') <> 'TRUE'").bind(start).bind(end).bind(field).bind(new),
    };
    Ok(q.fetch_one(&s.db).await?.try_get_unchecked::<i64, _>(0).unwrap_or(0))
}

/// The whole page for one day.
pub async fn get(_u: CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    let today = clock::now().date();
    let day = parse_day(q.get("date").map(String::as_str))?;
    let agents: Vec<(String, String, String)> = sqlx::query_as("SELECT agent, name, color FROM checklist_agents ORDER BY pos").fetch_all(&s.db).await?;
    let tasks: Vec<(String, String, String)> = sqlx::query_as("SELECT code, title, agent FROM checklist_tasks ORDER BY pos, code").fetch_all(&s.db).await?;
    let marks = sqlx::query("SELECT task, CAST(doing AS SIGNED), doing_by, CAST(done AS SIGNED), done_by, CAST(remarks AS SIGNED), remarks_by, number_text FROM checklist_marks WHERE day = ?")
        .bind(day)
        .fetch_all(&s.db)
        .await?;
    let mut m: HashMap<String, Value> = HashMap::new();
    for r in &marks {
        let t = |i: usize| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
        let b = |i: usize| r.try_get_unchecked::<i64, _>(i).unwrap_or(0) != 0;
        m.insert(t(0), json!({ "doing": b(1), "doing_by": t(2), "done": b(3), "done_by": t(4), "remarks": b(5), "remarks_by": t(6), "number_text": t(7) }));
    }
    // the counted numbers
    let mut n: HashMap<&str, Option<i64>> = HashMap::new();
    if day == today {
        n.insert("A", backlog(&s).await?.get("A").copied());
    } else {
        let a: Option<i64> = sqlx::query_scalar("SELECT n FROM checklist_counts WHERE day = ? AND task = 'A'").bind(day).fetch_optional(&s.db).await?;
        n.insert("A", a);
    }
    n.insert("B", Some(changes(&s, day, "source_order_status", Some("In Transit"), "Delivered").await?));
    n.insert("D", Some(changes(&s, day, "package_not_received", None, "TRUE").await?));
    n.insert("P", Some(changes(&s, day, "tracking_status", Some("In Transit"), "Delivered").await?));
    let (red, magenta) = left_colours(&s, day).await?;
    n.insert("F", Some(red));
    n.insert("G", Some(magenta));
    n.insert("O", Some(blanks_filled(&s, day).await?));

    let out: Vec<Value> = tasks
        .into_iter()
        .map(|(code, title, agent)| {
            let mut v = json!({ "code": code, "title": title, "agent": agent, "doing": false, "doing_by": "", "done": false, "done_by": "", "remarks": false, "remarks_by": "", "number_text": "" });
            if let (Some(obj), Some(mk)) = (v.as_object_mut(), m.get(&code).and_then(Value::as_object)) {
                obj.extend(mk.clone());
            }
            if let Some(note) = counted(&code) {
                v["counted"] = json!(true);
                v["how"] = json!(note);
                v["n"] = json!(n.get(code.as_str()).copied().flatten());
            }
            v
        })
        .collect();
    let items: Vec<(i32, String, String, String)> = sqlx::query_as("SELECT pos, CAST(kind AS CHAR), `text`, updated_by FROM checklist_items ORDER BY pos").fetch_all(&s.db).await?;
    let list = |kind: &str| items.iter().filter(|i| i.1 == kind).map(|(pos, _, text, by)| json!({ "pos": pos, "text": text, "by": by })).collect::<Vec<_>>();
    // the card: the orders whose Tracking Status is Preshipment, except the ones whose Source Order Status says they are not going to ship
    // (For Refund, Refunded, Cancelled, Unfulfilled). All Orders opens with the same two filters.
    let sch = s.schema();
    // (and only real orders: a row with no Store Order No. is an empty row that only holds a label number)
    let preshipment = sqlx::query(&format!(
        "SELECT COUNT(*) FROM orders_text WHERE {} = 'Preshipment' AND lower(trim({})) NOT IN ('for refund', 'refunded', 'cancelled', 'unfulfilled') AND trim({}) <> ''",
        sch.c("Tracking Status"),
        sch.c("Source Order Status"),
        sch.c("Store Order No.")
    ))
    .fetch_one(&s.db)
    .await?
    .try_get_unchecked::<i64, _>(0)
    .unwrap_or(0);
    Ok(Json(json!({
        "preshipment": preshipment,
        "date": day.format("%Y-%m-%d").to_string(),
        "today": today.format("%Y-%m-%d").to_string(),
        "agents": agents.into_iter().map(|(agent, name, color)| json!({ "agent": agent, "name": name, "color": color })).collect::<Vec<_>>(),
        "tasks": out,
        "additional": list("additional"),
        "sku": list("sku"),
    })))
}

async fn task_exists(s: &AppState, code: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM checklist_tasks WHERE code = ?").bind(code).fetch_one(&s.db).await? > 0)
}

fn text_of(b: &Value, key: &str) -> AppResult<String> {
    let t = b.get(key).and_then(Value::as_str).unwrap_or("").replace("\r\n", "\n");
    if t.chars().count() > MAX_TEXT {
        return Err(AppError::bad(format!("That is too long (at most {MAX_TEXT} characters).")));
    }
    Ok(t.trim_end().to_string())
}

/// Ticks (or unticks) Doing / Done / Remarks of one task on one day. The name of whoever ticked it is kept.
pub async fn mark(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let day = parse_day(b.get("date").and_then(Value::as_str))?;
    let task = b.get("task").and_then(Value::as_str).unwrap_or("");
    if !task_exists(&s, task).await? {
        return Err(AppError::bad("That task does not exist."));
    }
    let on = b.get("value").and_then(Value::as_bool).unwrap_or(false);
    let sql = match b.get("field").and_then(Value::as_str).unwrap_or("") {
        "doing" => "INSERT INTO checklist_marks (day, task, doing, doing_by) VALUES (?,?,?,?) ON DUPLICATE KEY UPDATE doing = VALUES(doing), doing_by = VALUES(doing_by)",
        "done" => "INSERT INTO checklist_marks (day, task, done, done_by) VALUES (?,?,?,?) ON DUPLICATE KEY UPDATE done = VALUES(done), done_by = VALUES(done_by)",
        "remarks" => "INSERT INTO checklist_marks (day, task, remarks, remarks_by) VALUES (?,?,?,?) ON DUPLICATE KEY UPDATE remarks = VALUES(remarks), remarks_by = VALUES(remarks_by)",
        _ => return Err(AppError::bad("Unknown box.")),
    };
    sqlx::query(sql).bind(day).bind(task).bind(on).bind(if on { u.name.as_str() } else { "" }).execute(&s.db).await?;
    s.bump();
    Ok(Json(json!({ "ok": true })))
}

/// The typed number of a task that the app does not count.
pub async fn number(CurrentUser(_u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let day = parse_day(b.get("date").and_then(Value::as_str))?;
    let task = b.get("task").and_then(Value::as_str).unwrap_or("");
    if !task_exists(&s, task).await? {
        return Err(AppError::bad("That task does not exist."));
    }
    if counted(task).is_some() {
        return Err(AppError::bad("The app counts this number by itself."));
    }
    let text = text_of(&b, "text")?;
    sqlx::query("INSERT INTO checklist_marks (day, task, number_text) VALUES (?,?,?) ON DUPLICATE KEY UPDATE number_text = VALUES(number_text)").bind(day).bind(task).bind(&text).execute(&s.db).await?;
    s.bump();
    Ok(Json(json!({ "ok": true })))
}

/// One cell of the Additional Task or SKU column, in the row `pos` (0 is the first row). An empty text clears the cell.
pub async fn cell(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let kind = b.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind != "additional" && kind != "sku" {
        return Err(AppError::bad("Unknown column."));
    }
    let pos = b.get("pos").and_then(Value::as_i64).filter(|p| (0..MAX_ROWS).contains(p)).ok_or_else(|| AppError::bad("That row does not exist."))?;
    let text = text_of(&b, "text")?;
    if text.trim().is_empty() {
        sqlx::query("DELETE FROM checklist_items WHERE kind = ? AND pos = ?").bind(kind).bind(pos).execute(&s.db).await?;
    } else {
        sqlx::query("INSERT INTO checklist_items (kind, pos, `text`, updated_by, updated_at) VALUES (?,?,?,?,?) ON DUPLICATE KEY UPDATE `text` = VALUES(`text`), updated_by = VALUES(updated_by), updated_at = VALUES(updated_at)")
            .bind(kind)
            .bind(pos)
            .bind(&text)
            .bind(&u.name)
            .bind(clock::now())
            .execute(&s.db)
            .await?;
    }
    s.bump();
    Ok(Json(json!({ "ok": true })))
}

/// An admin changes the words of a task, or who it is assigned to (its colour).
pub async fn task_edit(AdminUser(_u): AdminUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let code = b.get("code").and_then(Value::as_str).unwrap_or("");
    if !task_exists(&s, code).await? {
        return Err(AppError::bad("That task does not exist."));
    }
    if let Some(title) = b.get("title").and_then(Value::as_str) {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 400 {
            return Err(AppError::bad("Write the task (at most 400 characters)."));
        }
        sqlx::query("UPDATE checklist_tasks SET title = ? WHERE code = ?").bind(title).bind(code).execute(&s.db).await?;
    }
    if let Some(agent) = b.get("agent").and_then(Value::as_str) {
        if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM checklist_agents WHERE agent = ?").bind(agent).fetch_one(&s.db).await? == 0 {
            return Err(AppError::bad("Pick one of the agents of the legend."));
        }
        sqlx::query("UPDATE checklist_tasks SET agent = ? WHERE code = ?").bind(agent).bind(code).execute(&s.db).await?;
    }
    s.bump();
    Ok(Json(json!({ "ok": true })))
}
