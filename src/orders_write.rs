//! All Orders, writing: typing in a cell, adding, deleting and restoring rows, and the small helpers other tabs use
//! (a scan changes cells the same way). The pages send every cell as TEXT; this module turns it into the real column type
//! (a date, a number, a 1/0 tick) and keeps the edit history, the search text, the calculated columns and the shipping dates.

use std::collections::HashSet;

use axum::extract::State;
use axum::response::Json;
use chrono::{NaiveDate, NaiveDateTime, Timelike};
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::{MySql, Row, Transaction};

use crate::auth::{AdminUser, CurrentUser, User};
use crate::error::{AppError, AppResult};
use crate::schema::{Col, Schema};
use crate::{clock, settings, AppState};

pub type Tx<'a> = Transaction<'a, MySql>;

const TRUE_WORDS: [&str; 6] = ["true", "yes", "y", "1", "x", "checked"];
const FALSE_WORDS: [&str; 4] = ["false", "no", "n", "0"];
/// Ticking one of these stamps the date and time of the tick (clearing it clears the stamp) -> the column that holds the stamp.
const SHIP_STAMP: [(&str, &str); 3] = [("Shipped to Customer", "SHIPPED TO  CUSTOMER DATE"), ("Shipped to Amazon", "SHIPPED TO  AMAZON DATE"), ("ITEM ON HOLD", "Hold Date")];
/// Shipped to the customer and shipped to Amazon exclude each other: the newest tick wins.
const SHIP_OTHER: [(&str, &str); 2] = [("Shipped to Customer", "Shipped to Amazon"), ("Shipped to Amazon", "Shipped to Customer")];
/// The date column stamped by each shipping step -> the kind of ship event.
const SHIP_KINDS: [(&str, &str); 2] = [("SHIPPED TO  CUSTOMER DATE", "customer"), ("SHIPPED TO  AMAZON DATE", "amazon")];

// ---------------------------------------------------------------- text <-> typed values

fn month_number(m: &str) -> Option<u32> {
    ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|x| x.eq_ignore_ascii_case(m)).map(|i| i as u32 + 1)
}

/// "Mar 6, 2026" or "Mar 6, 2026 3:04 PM" -> "2026-03-06" / "2026-03-06 15:04"; anything else is returned as it is.
pub fn norm_date(s: &str) -> String {
    // built once (this runs for every date cell of an imported sheet)
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| Regex::new(r"^\s*([A-Za-z]{3})\s+(\d{1,2}),\s*(\d{4})(?:\s+(\d{1,2}):(\d{2})\s*(AM|PM))?\s*$").unwrap());
    let Some(m) = RE.captures(s) else { return s.to_string() };
    let Some(mo) = month_number(&m[1]) else { return s.to_string() };
    let mut out = format!("{:04}-{:02}-{:02}", m[3].parse::<u32>().unwrap_or(0), mo, m[2].parse::<u32>().unwrap_or(0));
    if let Some(h) = m.get(4) {
        let h = h.as_str().parse::<u32>().unwrap_or(0) % 12 + if &m[6] == "PM" { 12 } else { 0 };
        out.push_str(&format!(" {h:02}:{}", &m[5]));
    }
    out
}

fn is_iso(s: &str) -> bool {
    let ok = s.get(..10).is_some_and(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok());
    ok && (s.len() == 10 || (s.len() == 16 && s.as_bytes()[10] == b' ' && chrono::NaiveTime::parse_from_str(&s[11..], "%H:%M").is_ok()))
}

/// Normalise what someone typed into a cell; an Err holds a plain message if it does not fit.
pub fn clean_value(kind: &str, v: &str) -> Result<String, String> {
    let v = v.replace("\r\n", "\n");
    let v = v.trim();
    match kind {
        "bool" => {
            let w = v.to_lowercase();
            if TRUE_WORDS.contains(&w.as_str()) {
                Ok("TRUE".into())
            } else if FALSE_WORDS.contains(&w.as_str()) {
                Ok("FALSE".into())
            } else if w.is_empty() {
                Ok(String::new())
            } else {
                Err("a checkbox column only takes TRUE or FALSE".into())
            }
        }
        "num" if !v.is_empty() => {
            let f: f64 = v.replace(',', "").parse().map_err(|_| "this column only takes numbers".to_string())?;
            Ok(if f == f.trunc() && f.abs() < 1e15 { format!("{}", f as i64) } else { format!("{f}") })
        }
        "date" if !v.is_empty() => {
            let d = norm_date(v);
            if is_iso(&d) {
                Ok(d)
            } else {
                Err("use a date like Mar 6, 2026".into())
            }
        }
        _ => Ok(v.to_string()),
    }
}

pub enum Typed {
    Null,
    T(String),
    I(i64),
    D(NaiveDate),
    Dt(NaiveDateTime),
}

/// The text of a cell as the value of its real column.
pub fn typed(col: &Col, text: &str) -> Typed {
    let t = text.trim();
    if t.is_empty() {
        return Typed::Null;
    }
    match col.dtype.as_str() {
        "tinyint" => match t.to_ascii_uppercase().as_str() {
            "TRUE" => Typed::I(1),
            "FALSE" => Typed::I(0),
            _ => Typed::Null,
        },
        "int" | "bigint" | "smallint" => t.replace(',', "").parse::<f64>().map_or(Typed::Null, |f| Typed::I(f.round() as i64)),
        "date" => t.get(..10).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map_or(Typed::Null, Typed::D),
        "datetime" => NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M")
            .ok()
            .or_else(|| t.get(..10).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).and_then(|d| d.and_hms_opt(0, 0, 0)))
            .map_or(Typed::Null, Typed::Dt),
        _ => Typed::T(text.to_string()),
    }
}

/// "YYYY-MM-DD HH:MM" as a date and time.
fn minute(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").ok()
}

/// The current Eastern time, to the minute (every stamp in the app is kept to the minute).
pub fn now_minute() -> NaiveDateTime {
    clock::now().with_second(0).and_then(|d| d.with_nanosecond(0)).unwrap_or_else(clock::now)
}

fn stamp(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}

// ---------------------------------------------------------------- reading and writing one cell

/// The text of a cell as the pages see it, or None when the row does not exist.
pub async fn read_text(tx: &mut Tx<'_>, rn: u64, j: usize) -> AppResult<Option<String>> {
    let row = sqlx::query(&format!("SELECT c{j} FROM orders_text WHERE rn = ?")).bind(rn).fetch_optional(&mut **tx).await?;
    Ok(row.map(|r| r.try_get_unchecked::<String, _>(0).unwrap_or_default()))
}

/// Writes the text of one cell to the real column.
pub async fn write_field(tx: &mut Tx<'_>, sch: &Schema, rn: u64, j: usize, text: &str) -> AppResult<()> {
    let col = &sch.cols[j];
    if col.custom {
        if text.is_empty() {
            sqlx::query("DELETE FROM order_custom_values WHERE rn = ? AND column_id = ?").bind(rn).bind(col.id).execute(&mut **tx).await?;
        } else {
            sqlx::query("INSERT INTO order_custom_values (rn, column_id, `value`) VALUES (?,?,?) ON DUPLICATE KEY UPDATE `value` = VALUES(`value`)").bind(rn).bind(col.id).bind(text).execute(&mut **tx).await?;
        }
        return Ok(());
    }
    let sql = format!("UPDATE orders SET `{}` = ? WHERE rn = ?", col.field);
    let q = sqlx::query(&sql);
    let q = match typed(col, text) {
        Typed::Null => q.bind(None::<String>),
        Typed::T(s) => q.bind(s),
        Typed::I(i) => q.bind(i),
        Typed::D(d) => q.bind(d),
        Typed::Dt(d) => q.bind(d),
    };
    q.bind(rn).execute(&mut **tx).await?;
    Ok(())
}

pub async fn log_edit(tx: &mut Tx<'_>, at: NaiveDateTime, who: &str, rn: u64, field: &str, old: &str, new: &str, note: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO edit_log (at, who, rn, field, old_value, new_value, note) VALUES (?,?,?,?,?,?,?)")
        .bind(at)
        .bind(who)
        .bind(rn)
        .bind(field)
        .bind(old)
        .bind(new)
        .bind(note)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The text the search box looks in, rebuilt for these rows.
pub async fn refresh_search(tx: &mut Tx<'_>, sch: &Schema, rns: &[u64]) -> AppResult<()> {
    let idx = sch.search_idx();
    if idx.is_empty() {
        return Ok(());
    }
    let cols = idx.iter().map(|i| format!("c{i}")).collect::<Vec<_>>().join(", ");
    for rn in rns {
        let Some(row) = sqlx::query(&format!("SELECT {cols} FROM orders_text WHERE rn = ?")).bind(rn).fetch_optional(&mut **tx).await? else { continue };
        let text = (0..idx.len()).map(|i| row.try_get_unchecked::<String, _>(i).unwrap_or_default()).collect::<Vec<_>>().join(" ").to_lowercase();
        sqlx::query("UPDATE orders SET search_text = ? WHERE rn = ?").bind(text).bind(rn).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Fill every calculated column (Gift Note = Label Number + Customer Name) from its sources, for the given rows or for all rows.
pub async fn recompute_formulas(tx: &mut Tx<'_>, sch: &Schema, rns: Option<&[u64]>) -> AppResult<()> {
    for (target, sources) in crate::schema::FORMULAS {
        let (Some(t), true) = (sch.at(target).map(|i| &sch.cols[i]), sources.iter().all(|s| sch.at(s).is_some())) else { continue };
        if t.custom || sources.iter().any(|s| sch.cols[sch.at(s).unwrap()].custom) {
            continue;
        }
        let parts = sources.iter().map(|s| format!("COALESCE(`{}`, '')", sch.cols[sch.at(s).unwrap()].field)).collect::<Vec<_>>().join(", ");
        let set = format!("UPDATE orders SET `{}` = NULLIF(CONCAT({parts}), '')", t.field);
        match rns {
            None => {
                sqlx::query(&set).execute(&mut **tx).await?;
            }
            Some(list) => {
                for chunk in list.chunks(500) {
                    let sql = format!("{set} WHERE rn IN ({})", vec!["?"; chunk.len()].join(","));
                    let mut q = sqlx::query(&sql);
                    for rn in chunk {
                        q = q.bind(rn);
                    }
                    q.execute(&mut **tx).await?;
                }
            }
        }
    }
    Ok(())
}

/// Change cells of one order by header and put every real change in the edit history (cell, old value, new value, who, why).
/// (The Python app's `Orders.set`; the scan and the Amazon import write this way.)
pub async fn orders_set(tx: &mut Tx<'_>, sch: &Schema, who: &str, rn: u64, values: &[(&str, String)], note: &str) -> AppResult<()> {
    if values.is_empty() {
        return Ok(());
    }
    let mut before = vec![];
    for (h, _) in values {
        let j = sch.at(h).ok_or_else(|| AppError::bad(format!("the column {h} does not exist")))?;
        before.push(read_text(tx, rn, j).await?.unwrap_or_default());
    }
    for (h, v) in values {
        write_field(tx, sch, rn, sch.at(h).unwrap(), v).await?;
        if let Some((_, kind)) = SHIP_KINDS.iter().find(|(c, _)| c == h) {
            if !v.is_empty() {
                let at = minute(v).unwrap_or_else(now_minute);
                sqlx::query("INSERT INTO ship_events (rn, kind, at, operator, note) VALUES (?,?,?,?,?)").bind(rn).bind(*kind).bind(at).bind(who).bind(note).execute(&mut **tx).await?;
            }
        }
    }
    for ((h, v), old) in values.iter().zip(&before) {
        if old != v && *h != "Updated by" {
            let field = &sch.cols[sch.at(h).unwrap()].field;
            log_edit(tx, now_minute(), who, rn, field, old, v, note).await?;
        }
    }
    Ok(())
}

/// A row that is now Delivered is no longer waiting for items: untick ITEM ON HOLD, clear the Hold Date, take out the
/// partial-delivery note, and stamp the day it arrived. Returns the cells it changed ({header: new value}).
pub async fn delivered_cleanup(tx: &mut Tx<'_>, sch: &Schema, who: &str, rn: u64, note: &str) -> AppResult<Vec<(String, String)>> {
    let mut writes: Vec<(&str, String)> = vec![];
    if read_header(tx, sch, rn, "Delivered to Warehouse Date").await?.is_empty() {
        writes.push(("Delivered to Warehouse Date", stamp(now_minute()))); // the day it arrived
    }
    if read_header(tx, sch, rn, "ITEM ON HOLD").await? == "TRUE" {
        writes.push(("ITEM ON HOLD", "FALSE".into()));
        if !read_header(tx, sch, rn, "Hold Date").await?.is_empty() {
            writes.push(("Hold Date", String::new()));
        }
    }
    let notes = read_header(tx, sch, rn, "Notes").await?;
    if partial_note().is_match(&notes) {
        writes.push(("Notes", without_partial_note(&notes)));
    }
    if writes.is_empty() {
        return Ok(vec![]);
    }
    orders_set(tx, sch, who, rn, &writes, note).await?;
    Ok(writes.into_iter().map(|(h, v)| (h.to_string(), v)).collect())
}

pub async fn read_header(tx: &mut Tx<'_>, sch: &Schema, rn: u64, header: &str) -> AppResult<String> {
    match sch.at(header) {
        Some(j) => Ok(read_text(tx, rn, j).await?.unwrap_or_default()),
        None => Ok(String::new()),
    }
}

pub fn partial_note() -> Regex {
    Regex::new(r"(?m)^Partial delivery: [^\n]*(\n|$)").unwrap()
}

pub fn without_partial_note(notes: &str) -> String {
    partial_note().replace_all(notes, "").trim().to_string()
}

pub fn with_partial_note(notes: &str, received: i64, qty: i64) -> String {
    let rest = without_partial_note(notes);
    let line = format!("Partial delivery: {received} of {qty} items received");
    if rest.is_empty() {
        line
    } else {
        format!("{rest}\n{line}")
    }
}

// ---------------------------------------------------------------- locks (protected columns and rows)

pub struct Locks(pub Option<Value>);

impl Locks {
    pub async fn load(s: &AppState) -> AppResult<Self> {
        Ok(Self(settings::get(&s.db, "locks").await?))
    }

    fn def(&self, id: &str) -> Option<&Value> {
        self.0.as_ref()?.get("defs")?.get(id)
    }

    fn col_id(&self, j: usize) -> Option<String> {
        self.0.as_ref()?.pointer("/orders/cols")?.get(j.to_string())?.as_str().map(str::to_string)
    }

    fn row_id(&self, rn: u64) -> Option<String> {
        let rows = self.0.as_ref()?.pointer("/orders/rows")?.as_object()?;
        rows.iter().find(|(_, ranges)| ranges.as_array().is_some_and(|rs| rs.iter().any(|r| r[0].as_u64().unwrap_or(1) <= rn && rn <= r[1].as_u64().unwrap_or(0)))).map(|(id, _)| id.clone())
    }

    /// "" when this person may type in it, otherwise the reason that is shown to them.
    fn why(&self, actor: &User, id: Option<String>, what: &str) -> String {
        let Some(lock) = id.as_deref().and_then(|i| self.def(i)) else { return String::new() };
        let admin = actor.is_admin();
        match lock["mode"].as_str().unwrap_or("") {
            "admins" => if admin { String::new() } else { format!("This {what} is protected: only admins can edit it.") },
            "people" => {
                let listed = lock["users"].as_array().is_some_and(|u| u.iter().any(|x| x.as_str() == Some(&actor.username)));
                if admin || listed { String::new() } else { format!("This {what} is protected: only admins and the people an admin chose can edit it.") }
            }
            _ => {
                let note = lock["note"].as_str().filter(|n| !n.is_empty()).map(|n| format!(" {n}")).unwrap_or_default();
                format!("This {what} is protected: nobody can type in it.{note}")
            }
        }
    }

    /// The reason this person may not edit this cell of a return (columns by field name, rows by return id), or "".
    pub fn blocked_returns(&self, actor: &User, field: &str, rid: &str) -> String {
        let col = self.0.as_ref().and_then(|l| l.pointer("/returns/cols")).and_then(|c| c.get(field)).and_then(Value::as_str).map(str::to_string);
        let m = self.why(actor, col, "column");
        if !m.is_empty() {
            return m;
        }
        let row = self.0.as_ref().and_then(|l| l.pointer("/returns/rows")).and_then(Value::as_object).and_then(|rows| rows.iter().find(|(_, rids)| rids.as_array().is_some_and(|r| r.iter().any(|x| x.as_str() == Some(rid)))).map(|(id, _)| id.clone()));
        self.why(actor, row, "row")
    }

    /// The reason this person may not edit this cell, or "".
    pub fn blocked(&self, actor: &User, j: usize, rn: u64) -> String {
        let m = self.why(actor, self.col_id(j), "column");
        if m.is_empty() {
            self.why(actor, self.row_id(rn), "row")
        } else {
            m
        }
    }
}

// ---------------------------------------------------------------- dropdowns

/// If the cell has a dropdown, the matching choice (fixing the capitals), or an Err for a value that is not one.
async fn check_dropdown(tx: &mut Tx<'_>, rn: u64, field: &str, v: &str) -> Result<String, String> {
    let row = sqlx::query("SELECT CAST(l.options AS CHAR), l.strict FROM dropdown_cells d JOIN dropdown_lists l ON l.id = d.list_id WHERE d.rn = ? AND d.field = ?")
        .bind(rn)
        .bind(field)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = row else { return Ok(v.to_string()) };
    if v.is_empty() {
        return Ok(v.to_string());
    }
    let options: Vec<String> = serde_json::from_str(&row.try_get_unchecked::<String, _>(0).unwrap_or_default()).unwrap_or_default();
    if let Some(o) = options.iter().find(|o| o.to_lowercase() == v.to_lowercase()) {
        return Ok(o.clone());
    }
    if row.try_get::<i8, _>(1).unwrap_or(0) != 0 {
        return Err(format!("choose one of: {}", options.join(", ")));
    }
    Ok(v.to_string())
}

// ---------------------------------------------------------------- saving cells

/// Save a batch of cell edits atomically: [{rn, col, value}] -> {values: [normalised value, ...], also: [...], skipped, reason}.
pub async fn cells(CurrentUser(user): CurrentUser, State(s): State<AppState>, Json(body): Json<Value>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let locks = Locks::load(&s).await?;
    let items = body.get("edits").and_then(Value::as_array).cloned().unwrap_or_default();
    let now = now_minute();
    let now_text = stamp(now);
    let mut tx = s.db.begin().await?;
    let (mut values, mut also) = (vec![], vec![]);
    let (mut skipped, mut reason) = (0, String::new());
    let (mut touched, mut delivered, mut recalc, mut shipped_ticks): (HashSet<u64>, HashSet<u64>, HashSet<u64>, HashSet<u64>) = Default::default();
    let (mut note_events, mut stock_sorts): (Vec<crate::notify::NoteEvent>, Vec<(u64, String)>) = (vec![], vec![]);
    let formula_cols = sch.formula_cols();
    let formula_sources: HashSet<usize> = crate::schema::FORMULAS.iter().flat_map(|(_, src)| src.iter().filter_map(|x| sch.at(x))).collect();
    let search_idx = sch.search_idx();
    let updated_by = sch.at("Updated by");

    for it in &items {
        let rn = it.get("rn").and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok())).ok_or_else(|| AppError::bad("bad row"))?;
        let j = it.get("col").and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok())).ok_or_else(|| AppError::bad("unknown column"))? as usize;
        if !sch.active(j) {
            return Err(AppError::bad("unknown column"));
        }
        let col = &sch.cols[j];
        let mut why = locks.blocked(&user, j, rn);
        if why.is_empty() && formula_cols.contains(&j) {
            let src = crate::schema::FORMULAS.iter().find(|(t, _)| *t == col.header).map(|(_, s)| s.join(" + ")).unwrap_or_default();
            why = format!("{} is calculated ({src}). Change those columns instead.", col.header);
        }
        if !why.is_empty() {
            skipped += 1;
            if reason.is_empty() {
                reason = why;
            }
            values.push(json!(read_text(&mut tx, rn, j).await?.unwrap_or_default()));
            continue;
        }
        let raw = it.get("value").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| it.get("value").map(|v| if v.is_null() { String::new() } else { v.to_string() }).unwrap_or_default());
        let fail = |e: String| AppError::bad(format!("Row {rn}, {}: {e}", col.header));
        let v = clean_value(&col.kind, &raw).map_err(fail)?;
        let v = check_dropdown(&mut tx, rn, &col.field, &v).await.map_err(|e| AppError::bad(format!("Row {rn}, {}: {e}", col.header)))?;
        let Some(before) = read_text(&mut tx, rn, j).await? else { return Err(AppError::bad(format!("Row {rn} no longer exists. Refresh the page."))) };
        write_field(&mut tx, &sch, rn, j, &v).await?;
        let changed = before != v;
        if changed && Some(j) != updated_by {
            log_edit(&mut tx, now, &user.name, rn, &col.field, &before, &v, "Edited").await?;
        }
        sqlx::query("INSERT INTO order_edits (rn, field, `value`, at) VALUES (?,?,?,?) ON DUPLICATE KEY UPDATE `value` = VALUES(`value`), at = VALUES(at)").bind(rn).bind(&col.field).bind(&v).bind(now).execute(&mut *tx).await?;
        values.push(json!(v));
        let h = col.header.as_str();
        if h == "Notes" && !v.is_empty() && changed {
            let label = read_header(&mut tx, &sch, rn, "Label Number").await?;
            let customer = read_header(&mut tx, &sch, rn, "Customer Name").await?;
            note_events.push(crate::notify::NoteEvent { rn, label, customer, field: h.to_string(), text: v.clone(), old: before.clone() }); // the bell tells the others
        }
        if h == "STOCK NO." && !v.is_empty() && changed {
            stock_sorts.push((rn, v.clone())); // counted on the Dashboard's Stock Sorting table
        }
        if h == "Source Order Status" && v == "Delivered" && changed {
            delivered.insert(rn);
        }
        if v == "TRUE" && changed && (h == "Shipped to Customer" || h == "Shipped to Amazon") {
            shipped_ticks.insert(rn);
        }
        // ticking Shipped to Customer / Shipped to Amazon stamps the date and time of the tick (clearing it clears the stamp)
        if let Some((_, stamp_header)) = SHIP_STAMP.iter().find(|(c, _)| *c == h) {
            if let (Some(sj), true) = (sch.at(stamp_header), (v == "TRUE" || v == "FALSE") && changed) {
                let st = if v == "TRUE" { now_text.clone() } else { String::new() };
                let old = read_text(&mut tx, rn, sj).await?.unwrap_or_default();
                write_field(&mut tx, &sch, rn, sj, &st).await?;
                if old != st {
                    let note = if st.is_empty() { "Cleared when the checkbox was unticked" } else { "Set when the checkbox was ticked" };
                    log_edit(&mut tx, now, &user.name, rn, &sch.cols[sj].field, &old, &st, note).await?;
                }
                also.push(json!({ "rn": rn, "col": sj, "value": st }));
                if !st.is_empty() && (h == "Shipped to Customer" || h == "Shipped to Amazon") {
                    let kind = if h == "Shipped to Customer" { "customer" } else { "amazon" };
                    sqlx::query("INSERT INTO ship_events (rn, kind, at, operator, note) VALUES (?,?,?,?,?)").bind(rn).bind(kind).bind(now).bind(&user.name).bind("Ticked by hand").execute(&mut *tx).await?;
                }
            }
        }
        // the newest event wins: shipped to the customer and shipped to Amazon exclude each other
        if let Some((_, other)) = SHIP_OTHER.iter().find(|(c, _)| *c == h) {
            if v == "TRUE" && changed {
                if let Some(oj) = sch.at(other) {
                    if read_text(&mut tx, rn, oj).await?.unwrap_or_default() == "TRUE" {
                        write_field(&mut tx, &sch, rn, oj, "FALSE").await?; // the date stays: it still counts as shipped
                        log_edit(&mut tx, now, &user.name, rn, &sch.cols[oj].field, "TRUE", "FALSE", &format!("Unticked: {h} was ticked")).await?;
                        also.push(json!({ "rn": rn, "col": oj, "value": "FALSE" }));
                    }
                }
            }
        }
        if search_idx.contains(&j) {
            touched.insert(rn);
        }
        if formula_sources.contains(&j) {
            recalc.insert(rn);
        }
    }

    // Delivered: no longer on hold, and the partial-delivery note goes
    for rn in &delivered {
        for (h, val) in delivered_cleanup(&mut tx, &sch, &user.name, *rn, "Row became Delivered").await? {
            if let Some(cj) = sch.at(&h) {
                also.push(json!({ "rn": rn, "col": cj, "value": val }));
            }
            touched.insert(*rn);
        }
    }
    if !recalc.is_empty() {
        let list: Vec<u64> = recalc.iter().copied().collect();
        recompute_formulas(&mut tx, &sch, Some(&list)).await?;
    }
    // "Updated by" names the operator who scanned the package, and the Dashboard counts by it. A scan sets it; so does ticking
    // Shipped to Customer / Shipped to Amazon by hand, which does the same job. Other edits leave it alone.
    if let Some(ub) = updated_by {
        for rn in &shipped_ticks {
            write_field(&mut tx, &sch, *rn, ub, &user.name).await?;
        }
    }
    let touched: Vec<u64> = touched.into_iter().collect();
    refresh_search(&mut tx, &sch, &touched).await?;
    tx.commit().await?;
    s.bump();

    if !note_events.is_empty() {
        crate::notify::add(&s, &user, &note_events).await?;
        s.bump();
    }
    if !stock_sorts.is_empty() {
        for (rn, v) in &stock_sorts {
            sqlx::query("INSERT INTO stock_sorts (at, operator, source, ref, stock_no) VALUES (?,?,?,?,?)").bind(now).bind(&user.name).bind("order").bind(rn.to_string()).bind(v).execute(&s.db).await?;
        }
        s.bump();
    }
    Ok(Json(json!({ "values": values, "also": also, "v": s.version_now(), "skipped": skipped, "reason": reason })))
}

// ---------------------------------------------------------------- rows

async fn top_rn(tx: &mut Tx<'_>) -> AppResult<u64> {
    let a: Option<u64> = sqlx::query_scalar("SELECT MAX(rn) FROM orders").fetch_one(&mut **tx).await?;
    let b: Option<u64> = sqlx::query_scalar("SELECT MAX(rn) FROM trash_rows").fetch_one(&mut **tx).await?;
    Ok(a.unwrap_or(2).max(b.unwrap_or(2)))
}

/// New rows at the bottom. Each row is {column number: value}; columns not mentioned start empty.
async fn add_rows(s: &AppState, user: &User, rows: Vec<Value>) -> AppResult<Value> {
    let sch = s.schema();
    let locks = Locks::load(s).await?;
    let rows: Vec<&Map<String, Value>> = rows.iter().filter_map(Value::as_object).take(500).collect();
    if rows.is_empty() {
        return Err(AppError::bad("No rows to add."));
    }
    let now = now_minute();
    let mut tx = s.db.begin().await?;
    let mut top = top_rn(&mut tx).await?;
    // a new row gets the dropdown that most of the column's cells have
    let mut lists: Vec<(String, u32)> = vec![];
    for (field, lid, _n) in sqlx::query_as::<_, (String, u32, i64)>("SELECT CAST(field AS CHAR), list_id, COUNT(*) n FROM dropdown_cells GROUP BY field, list_id ORDER BY n").fetch_all(&mut *tx).await? {
        lists.retain(|(f, _)| *f != field);
        lists.push((field, lid));
    }
    let formula_cols = sch.formula_cols();
    let mut new = vec![];
    for given in rows {
        top += 1;
        sqlx::query("INSERT INTO orders (rn) VALUES (?)").bind(top).execute(&mut *tx).await?;
        let mut typed_cells: Vec<(usize, String)> = vec![];
        for (k, v) in given {
            let Ok(j) = k.parse::<usize>() else { continue };
            let text = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            if !sch.active(j) || text.trim().is_empty() || formula_cols.contains(&j) {
                continue;
            }
            let why = locks.why(user, locks.col_id(j), "column");
            if !why.is_empty() {
                return Err(AppError::bad(format!("{why} ({})", sch.cols[j].header)));
            }
            let clean = clean_value(&sch.cols[j].kind, &text).map_err(|e| AppError::bad(format!("{}: {e}", sch.cols[j].header)))?;
            typed_cells.push((j, clean));
        }
        if let Some(ub) = sch.at("Updated by") {
            write_field(&mut tx, &sch, top, ub, &user.name).await?;
        }
        for (field, lid) in &lists {
            if sch.cols.iter().any(|c| &c.field == field && !c.deleted) {
                sqlx::query("INSERT IGNORE INTO dropdown_cells (rn, field, list_id) VALUES (?,?,?)").bind(top).bind(field).bind(lid).execute(&mut *tx).await?;
            }
        }
        for (j, v) in typed_cells {
            let v = check_dropdown(&mut tx, top, &sch.cols[j].field, &v).await.map_err(|e| AppError::bad(format!("{}: {e}", sch.cols[j].header)))?; // a dropdown column only takes its choices
            write_field(&mut tx, &sch, top, j, &v).await?;
            log_edit(&mut tx, now, &user.name, top, &sch.cols[j].field, "", &v, "Row added").await?;
        }
        new.push(top);
    }
    recompute_formulas(&mut tx, &sch, Some(&new)).await?;
    refresh_search(&mut tx, &sch, &new).await?;
    tx.commit().await?;
    s.bump();
    Ok(json!({ "rns": new }))
}

pub async fn rows(CurrentUser(user): CurrentUser, State(s): State<AppState>, axum::extract::Path(action): axum::extract::Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    if action == "add" {
        return Ok(Json(add_rows(&s, &user, b.get("rows").and_then(Value::as_array).cloned().unwrap_or_default()).await?));
    }
    // delete and restore change what everybody sees: admins only
    if !user.is_admin() {
        return Err(AppError::Forbidden("Only admins can do this.".into()));
    }
    let sch = s.schema();
    let mut rns: Vec<u64> = b.get("rns").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_u64().or_else(|| x.as_str()?.parse().ok())).collect()).unwrap_or_default();
    rns.sort_unstable();
    rns.dedup();
    let everything = action == "restore" && b.get("all").and_then(Value::as_bool).unwrap_or(false);
    if rns.is_empty() && !everything {
        return Err(AppError::bad("No rows chosen."));
    }
    let mut tx = s.db.begin().await?;
    let mut n = 0;
    match action.as_str() {
        "delete" => {
            let at = now_minute();
            let cols: Vec<String> = (0..sch.n()).map(|i| format!("c{i}")).collect();
            for rn in &rns {
                let Some(row) = sqlx::query(&format!("SELECT {}, _s FROM orders_text WHERE rn = ?", cols.join(", "))).bind(rn).fetch_optional(&mut *tx).await? else { continue };
                let vals: Vec<String> = (0..sch.n()).map(|i| row.try_get_unchecked::<String, _>(i).unwrap_or_default()).collect();
                let search = row.try_get_unchecked::<String, _>(sch.n()).unwrap_or_default();
                let dv: Vec<(String, u32)> = sqlx::query_as("SELECT CAST(field AS CHAR), list_id FROM dropdown_cells WHERE rn = ?").bind(rn).fetch_all(&mut *tx).await?;
                let data = json!({ "c": vals, "s": search, "dv": dv });
                let label = sch.at("Label Number").map(|i| vals[i].clone()).unwrap_or_default();
                let product = sch.at("Product Name").map(|i| vals[i].clone()).unwrap_or_default();
                let customer = sch.at("Customer Name").map(|i| vals[i].clone()).unwrap_or_default();
                sqlx::query("REPLACE INTO trash_rows (rn, label, product, customer, data, deleted_by, deleted_at) VALUES (?,?,?,?,?,?,?)")
                    .bind(rn).bind(label).bind(product).bind(customer).bind(data.to_string()).bind(&user.name).bind(at).execute(&mut *tx).await?;
                sqlx::query("DELETE FROM orders WHERE rn = ?").bind(rn).execute(&mut *tx).await?;
                n += 1;
            }
        }
        "restore" => {
            if everything {
                rns = sqlx::query_scalar("SELECT rn FROM trash_rows ORDER BY rn").fetch_all(&mut *tx).await?;
            }
            let mut top = top_rn(&mut tx).await?;
            for rn in &rns {
                let Some(t) = sqlx::query_scalar::<_, String>("SELECT CAST(data AS CHAR) FROM trash_rows WHERE rn = ?").bind(rn).fetch_optional(&mut *tx).await? else { continue };
                let d: Value = serde_json::from_str(&t).unwrap_or(json!({}));
                let vals: Vec<String> = d["c"].as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default(); // columns added since come back empty
                let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orders WHERE rn = ?").bind(rn).fetch_one(&mut *tx).await?;
                let new_rn = if exists > 0 { top += 1; top } else { *rn };
                sqlx::query("INSERT INTO orders (rn, search_text) VALUES (?, ?)").bind(new_rn).bind(d["s"].as_str().unwrap_or("")).execute(&mut *tx).await?;
                for (j, v) in vals.iter().enumerate().take(sch.n()) {
                    if !v.is_empty() && sch.active(j) {
                        write_field(&mut tx, &sch, new_rn, j, v).await?;
                    }
                }
                for dv in d["dv"].as_array().into_iter().flatten() {
                    if let (Some(f), Some(l)) = (dv[0].as_str(), dv[1].as_u64()) {
                        sqlx::query("INSERT IGNORE INTO dropdown_cells (rn, field, list_id) SELECT ?, ?, id FROM dropdown_lists WHERE id = ?").bind(new_rn).bind(f).bind(l).execute(&mut *tx).await?;
                    }
                }
                sqlx::query("DELETE FROM trash_rows WHERE rn = ?").bind(rn).execute(&mut *tx).await?;
                n += 1;
            }
        }
        _ => return Err(AppError::NotFound),
    }
    tx.commit().await?;
    s.bump();
    Ok(Json(json!({ "count": n })))
}

/// The deleted rows (and deleted columns) an admin can still restore.
pub async fn trash(_a: AdminUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let rows = sqlx::query_as::<_, (u64, Option<String>, Option<String>, Option<String>, String, NaiveDateTime)>(
        "SELECT rn, CAST(label AS CHAR), CAST(product AS CHAR), CAST(customer AS CHAR), deleted_by, deleted_at FROM trash_rows ORDER BY deleted_at DESC, rn DESC LIMIT 500",
    )
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trash_rows").fetch_one(&s.db).await?;
    let rows: Vec<Value> = rows.into_iter().map(|(rn, label, product, customer, by, at)| json!({ "rn": rn, "label": label.unwrap_or_default(), "product": product.unwrap_or_default(), "customer": customer.unwrap_or_default(), "by": by, "at": stamp(at) })).collect();
    let cols_meta = sqlx::query_as::<_, (u64, Option<String>, Option<NaiveDateTime>)>("SELECT CAST(id AS UNSIGNED), deleted_by, deleted_at FROM column_defs WHERE deleted = 1").fetch_all(&s.db).await?;
    let columns: Vec<Value> = sch
        .cols
        .iter()
        .filter(|c| c.deleted)
        .map(|c| {
            let meta = cols_meta.iter().find(|m| m.0 == c.id);
            // the header keeps its "~deleted n~" prefix while the column is in the trash
            let name = c.header.split_once("~ ").map(|(_, n)| n.to_string()).unwrap_or_else(|| c.header.clone());
            json!({ "i": c.i, "name": name, "by": meta.and_then(|m| m.1.clone()).unwrap_or_default(), "at": meta.and_then(|m| m.2).map(stamp).unwrap_or_default(), "custom": c.custom })
        })
        .collect();
    Ok(Json(json!({ "rows": rows, "total": total, "columns": columns })))
}
