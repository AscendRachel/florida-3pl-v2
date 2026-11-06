//! Returns (the sheet's RETURNS tab), the Kohl's list and Materials: three lists the warehouse keeps.
//!
//! Returns come from a master returns sheet shared with "anyone with the link", keeping only the rows of this warehouse; the app reads
//! that sheet by itself on a timer. What the warehouse does with a return (Shipped to Amazon, who processed it and when, its own
//! notes) lives only here, so a sync never overwrites it; what people type over a sheet value is remembered separately (`ovr`).
//! The pages see every value as TEXT ("" for blank, "TRUE" / "FALSE" for a tick); the database keeps real types.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::Json;
use chrono::{NaiveDate, NaiveDateTime};
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::auth::{CurrentUser, User};
use crate::error::{AppError, AppResult};
use crate::orders_write::{now_minute, Locks};
use crate::{clock, settings, AppState};

fn stamp(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn ival(b: &Value, k: &str) -> i64 {
    match b.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn clip(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

/// A decimal as the text a person would type: 10.00 -> "10", 10.50 -> "10.5".
fn plain_number(s: &str) -> String {
    s.parse::<f64>().map_or_else(|_| s.to_string(), |f| if f == f.trunc() { format!("{}", f as i64) } else { format!("{f}") })
}

// ================================================================ Materials

const MAT_TEXT: [(&str, usize); 11] = [("mid", 20), ("image", 600), ("name", 200), ("spec", 200), ("unit", 30), ("pack", 60), ("ordered", 12), ("current", 12), ("alert", 12), ("minimum", 12), ("notes", 2000)];
const MAT_NUMBERS: [&str; 4] = ["ordered", "current", "alert", "minimum"];
const UNITS: [&str; 4] = ["Pack", "Roll", "Box", "Each"];

/// A material as the page sees it (every value as text).
const MAT_SELECT: &str = "SELECT id, CAST(IFNULL(mid,'') AS CHAR), CAST(IFNULL(image,'') AS CHAR), CAST(IFNULL(name,'') AS CHAR), CAST(IFNULL(spec,'') AS CHAR), CAST(IFNULL(unit,'') AS CHAR), \
     CAST(IFNULL(pack,'') AS CHAR), IFNULL(CAST(ordered AS CHAR),''), IFNULL(CAST(`current` AS CHAR),''), IFNULL(CAST(alert AS CHAR),''), IFNULL(CAST(minimum AS CHAR),''), CAST(IFNULL(status,'') AS CHAR), \
     IFNULL(DATE_FORMAT(updated_at, '%Y-%m-%d %H:%i:%s'),''), CAST(IFNULL(updated_by,'') AS CHAR), CAST(IFNULL(notes,'') AS CHAR) FROM materials";
const MAT_KEYS: [&str; 15] = ["id", "mid", "image", "name", "spec", "unit", "pack", "ordered", "current", "alert", "minimum", "status", "updated_at", "updated_by", "notes"];

fn mat_row(r: &sqlx::mysql::MySqlRow) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(r.try_get::<u64, _>(0).unwrap_or(0)));
    for (i, k) in MAT_KEYS.iter().enumerate().skip(1) {
        let t = r.try_get_unchecked::<String, _>(i).unwrap_or_default();
        m.insert((*k).into(), json!(if MAT_NUMBERS.contains(k) { plain_number(&t) } else { t }));
    }
    Value::Object(m)
}

async fn mat_one(s: &AppState, id: u64) -> AppResult<Value> {
    let r = sqlx::query(&format!("{MAT_SELECT} WHERE id = ?")).bind(id).fetch_one(&s.db).await?;
    Ok(mat_row(&r))
}

fn num(v: &str) -> Option<f64> {
    v.replace(',', "").trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

/// The Stock Status is worked out from the numbers, so it is never typed.
fn status_of(current: &str, alert: &str, minimum: &str) -> &'static str {
    let Some(cur) = num(current) else { return "" };
    if cur <= 0.0 {
        "OUT OF STOCK"
    } else if num(minimum).is_some_and(|m| cur <= m) {
        "CRITICAL"
    } else if num(alert).is_some_and(|a| cur <= a) {
        "REORDER ALERT"
    } else {
        "GOOD"
    }
}

fn mat_clean(field: &str, value: &Value) -> AppResult<String> {
    let Some((_, n)) = MAT_TEXT.iter().find(|(f, _)| *f == field) else { return Err(AppError::bad("That column cannot be edited.")) };
    let v = clip(&text_of(value), *n);
    if MAT_NUMBERS.contains(&field) && !v.is_empty() && num(&v).is_none() {
        let name: String = field.chars().enumerate().map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c }).collect();
        return Err(AppError::bad(format!("{name} must be a number.")));
    }
    Ok(v)
}

async fn mat_set(tx: &mut crate::orders_write::Tx<'_>, id: u64, field: &str, v: &str) -> AppResult<()> {
    let sql = format!("UPDATE materials SET `{field}` = ? WHERE id = ?");
    let q = sqlx::query(&sql);
    let q = if MAT_NUMBERS.contains(&field) { q.bind(num(v)) } else { q.bind(v.to_string()) };
    q.bind(id).execute(&mut **tx).await?;
    Ok(())
}

async fn mat_refresh(tx: &mut crate::orders_write::Tx<'_>, id: u64, operator: &str, touch: bool) -> AppResult<()> {
    let r = sqlx::query("SELECT IFNULL(CAST(`current` AS CHAR),''), IFNULL(CAST(alert AS CHAR),''), IFNULL(CAST(minimum AS CHAR),'') FROM materials WHERE id = ?").bind(id).fetch_one(&mut **tx).await?;
    let t = |i| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
    sqlx::query("UPDATE materials SET status = ? WHERE id = ?").bind(status_of(&t(0), &t(1), &t(2))).bind(id).execute(&mut **tx).await?;
    if touch {
        sqlx::query("UPDATE materials SET updated_at = ?, updated_by = ? WHERE id = ?").bind(clock::now()).bind(operator).bind(id).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn mat_add(tx: &mut crate::orders_write::Tx<'_>, operator: &str, values: &Value) -> AppResult<u64> {
    let mut v: HashMap<&str, String> = MAT_TEXT.iter().map(|(k, _)| (*k, String::new())).collect();
    if let Some(m) = values.as_object() {
        for (k, val) in m {
            if let Some((key, _)) = MAT_TEXT.iter().find(|(f, _)| f == k) {
                v.insert(key, mat_clean(key, val)?);
            }
        }
    }
    if v["mid"].is_empty() {
        // the next MAT-001 style number
        let mids: Vec<String> = sqlx::query_scalar("SELECT CAST(mid AS CHAR) FROM materials").fetch_all(&mut **tx).await?;
        let re = Regex::new(r"^MAT-(\d+)$").unwrap();
        let top = mids.iter().filter_map(|m| re.captures(m).and_then(|c| c[1].parse::<u64>().ok())).max().unwrap_or(0);
        v.insert("mid", format!("MAT-{:03}", top + 1));
    }
    let id = sqlx::query("INSERT INTO materials (mid, image, name, spec, unit, pack, notes) VALUES (?,?,?,?,?,?,?)")
        .bind(&v["mid"]).bind(&v["image"]).bind(&v["name"]).bind(&v["spec"]).bind(&v["unit"]).bind(&v["pack"]).bind(&v["notes"])
        .execute(&mut **tx).await?.last_insert_id();
    for f in MAT_NUMBERS {
        mat_set(tx, id, f, &v[f]).await?;
    }
    mat_refresh(tx, id, operator, true).await?;
    Ok(id)
}

pub async fn materials_get(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query(&format!("{MAT_SELECT} ORDER BY id")).fetch_all(&s.db).await?;
    let mut units: std::collections::BTreeSet<String> = UNITS.iter().map(|u| u.to_string()).collect();
    units.extend(sqlx::query_scalar::<_, String>("SELECT DISTINCT CAST(unit AS CHAR) FROM materials WHERE unit <> ''").fetch_all(&s.db).await?);
    Ok(Json(json!({ "rows": rows.iter().map(mat_row).collect::<Vec<_>>(), "units": units })))
}

pub async fn materials_post(CurrentUser(u): CurrentUser, State(s): State<AppState>, Path(action): Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    if action == "delete" && !u.is_admin() {
        return Err(AppError::Forbidden("Only admins can do this.".into()));
    }
    let mut tx = s.db.begin().await?;
    let mut out = Map::new();
    match action.as_str() {
        "add" => {
            let id = mat_add(&mut tx, &u.name, b.get("values").unwrap_or(&Value::Null)).await?;
            tx.commit().await?;
            out.insert("row".into(), mat_one(&s, id).await?);
        }
        "edit" => {
            let id = ival(&b, "id") as u64;
            let field = text_of(&b["field"]);
            if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM materials WHERE id = ?").bind(id).fetch_one(&mut *tx).await? == 0 {
                return Err(AppError::bad("That row no longer exists."));
            }
            let v = mat_clean(&field, &b["value"])?; // refuses a column that cannot be edited, so the name below is always one of MAT_TEXT
            mat_set(&mut tx, id, &field, &v).await?;
            mat_refresh(&mut tx, id, &u.name, field != "notes").await?;
            tx.commit().await?;
            out.insert("row".into(), mat_one(&s, id).await?);
        }
        "bulk" => {
            // a paste: cells of existing rows ({id, field, value}) and whole new rows
            let mut ids: Vec<u64> = vec![];
            for e in b["edits"].as_array().into_iter().flatten().take(5000) {
                let (f, id) = (text_of(&e["field"]), ival(e, "id") as u64);
                let v = mat_clean(&f, &e["value"])?;
                mat_set(&mut tx, id, &f, &v).await?;
                mat_refresh(&mut tx, id, &u.name, f != "notes").await?;
                ids.push(id);
            }
            for values in b["new"].as_array().into_iter().flatten().take(5000) {
                ids.push(mat_add(&mut tx, &u.name, values).await?);
            }
            tx.commit().await?;
            let mut seen = HashSet::new();
            let mut rows = vec![];
            for id in ids.into_iter().filter(|i| seen.insert(*i)) {
                if let Ok(r) = mat_one(&s, id).await {
                    rows.push(r);
                }
            }
            out.insert("rows".into(), json!(rows));
        }
        "delete" => {
            sqlx::query("DELETE FROM materials WHERE id = ?").bind(ival(&b, "id") as u64).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        _ => return Err(AppError::NotFound),
    }
    s.bump();
    out.insert("v".into(), json!(s.version_now()));
    Ok(Json(Value::Object(out)))
}

// ================================================================ Kohl's

const KOHLS_STORES: [&str; 3] = ["KOHLS GREENTEK", "KOHLS JESS", "KOHLS ACPJR"];
const KOHLS_TEXT: [(&str, usize); 8] = [("store", 60), ("stock_no", 60), ("sku", 80), ("order_number", 60), ("product", 400), ("image", 600), ("qty", 12), ("notes", 2000)];
const KOHLS_BOOL: [&str; 2] = ["good", "used"];
const KOHLS_SELECT: &str = "SELECT id, CAST(store AS CHAR), CAST(stock_no AS CHAR), CAST(sku AS CHAR), CAST(order_number AS CHAR), CAST(IFNULL(product,'') AS CHAR), CAST(IFNULL(image,'') AS CHAR), \
     IFNULL(CAST(qty AS CHAR),''), IF(good = 1, 'TRUE', 'FALSE'), IF(used = 1, 'TRUE', 'FALSE'), CAST(IFNULL(notes,'') AS CHAR), CAST(updated_by AS CHAR), IFNULL(DATE_FORMAT(updated_at, '%Y-%m-%d %H:%i'),'') FROM kohls";
const KOHLS_KEYS: [&str; 13] = ["id", "store", "stock_no", "sku", "order_number", "product", "image", "qty", "good", "used", "notes", "updated_by", "updated_at"];

fn kohls_row(r: &sqlx::mysql::MySqlRow) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(r.try_get::<u64, _>(0).unwrap_or(0)));
    for (i, k) in KOHLS_KEYS.iter().enumerate().skip(1) {
        m.insert((*k).into(), json!(r.try_get_unchecked::<String, _>(i).unwrap_or_default()));
    }
    Value::Object(m)
}

async fn kohls_one(s: &AppState, id: u64) -> AppResult<Value> {
    Ok(kohls_row(&sqlx::query(&format!("{KOHLS_SELECT} WHERE id = ?")).bind(id).fetch_one(&s.db).await?))
}

fn kohls_bool(v: &Value) -> bool {
    matches!(text_of(v).trim().to_lowercase().as_str(), "true" | "yes" | "y" | "1" | "checked" | "x" | "✓")
}

fn kohls_clean(field: &str, v: &Value) -> AppResult<Value> {
    if KOHLS_BOOL.contains(&field) {
        Ok(json!(kohls_bool(v) as i64))
    } else if let Some((_, n)) = KOHLS_TEXT.iter().find(|(f, _)| *f == field) {
        let t = clip(&text_of(v), *n);
        Ok(if field == "qty" { num(&t).map_or(Value::Null, |f| json!(f.round() as i64)) } else { json!(t) })
    } else {
        Err(AppError::bad("That column cannot be edited."))
    }
}

async fn log_sort(tx: &mut crate::orders_write::Tx<'_>, operator: &str, source: &str, reference: u64, stock_no: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO stock_sorts (at, operator, source, ref, stock_no) VALUES (?,?,?,?,?)").bind(now_minute()).bind(operator).bind(source).bind(reference.to_string()).bind(stock_no).execute(&mut **tx).await?;
    Ok(())
}

async fn kohls_set(tx: &mut crate::orders_write::Tx<'_>, id: u64, field: &str, v: &Value, operator: &str) -> AppResult<()> {
    // a Kohl's STOCK NO. that is now filled in and was not that value before is one stock sort
    if field == "stock_no" && !text_of(v).is_empty() {
        let old: Option<String> = sqlx::query_scalar("SELECT CAST(stock_no AS CHAR) FROM kohls WHERE id = ?").bind(id).fetch_optional(&mut **tx).await?;
        if old.is_some_and(|o| o != text_of(v)) {
            log_sort(tx, operator, "kohls", id, &text_of(v)).await?;
        }
    }
    let sql = format!("UPDATE kohls SET `{field}` = ?, updated_by = ?, updated_at = ? WHERE id = ?");
    let q = sqlx::query(&sql);
    let q = match v {
        Value::Number(n) => q.bind(n.as_i64().unwrap_or(0)),
        Value::Null => q.bind(None::<i64>),
        other => q.bind(text_of(other)),
    };
    q.bind(operator).bind(now_minute()).bind(id).execute(&mut **tx).await?;
    Ok(())
}

async fn kohls_add(tx: &mut crate::orders_write::Tx<'_>, operator: &str, values: &Value) -> AppResult<u64> {
    let get = |k: &str| values.get(k).cloned().unwrap_or(Value::Null);
    let text = |k: &str| kohls_clean(k, &get(k)).map(|v| text_of(&v)).unwrap_or_default();
    let qty = kohls_clean("qty", &get("qty"))?;
    let id = sqlx::query("INSERT INTO kohls (store, stock_no, sku, order_number, product, image, qty, good, used, notes, updated_by, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(text("store")).bind(text("stock_no")).bind(text("sku")).bind(text("order_number")).bind(text("product")).bind(text("image"))
        .bind(qty.as_i64()).bind(kohls_bool(&get("good")) as i64).bind(kohls_bool(&get("used")) as i64).bind(text("notes")).bind(operator).bind(now_minute())
        .execute(&mut **tx).await?.last_insert_id();
    if !text("stock_no").is_empty() {
        log_sort(tx, operator, "kohls", id, &text("stock_no")).await?;
    }
    Ok(id)
}

pub async fn kohls_get(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query(&format!("{KOHLS_SELECT} ORDER BY id")).fetch_all(&s.db).await?;
    Ok(Json(json!({ "rows": rows.iter().map(kohls_row).collect::<Vec<_>>(), "stores": KOHLS_STORES })))
}

pub async fn kohls_post(CurrentUser(u): CurrentUser, State(s): State<AppState>, Path(action): Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    if action == "delete" && !u.is_admin() {
        return Err(AppError::Forbidden("Only admins can do this.".into()));
    }
    let mut tx = s.db.begin().await?;
    let mut out = Map::new();
    match action.as_str() {
        "add" => {
            let id = kohls_add(&mut tx, &u.name, b.get("values").unwrap_or(&Value::Null)).await?;
            tx.commit().await?;
            out.insert("row".into(), kohls_one(&s, id).await?);
        }
        "edit" => {
            let (id, field) = (ival(&b, "id") as u64, text_of(&b["field"]));
            if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM kohls WHERE id = ?").bind(id).fetch_one(&mut *tx).await? == 0 {
                return Err(AppError::bad("That row no longer exists."));
            }
            let v = if KOHLS_BOOL.contains(&field.as_str()) {
                let on = matches!(&b["value"], Value::Bool(true)) || matches!(&b["value"], Value::String(x) if x == "TRUE" || x == "true") || b["value"] == json!(1);
                json!(on as i64)
            } else {
                kohls_clean(&field, &b["value"])?
            };
            kohls_set(&mut tx, id, &field, &v, &u.name).await?;
            tx.commit().await?;
            out.insert("row".into(), kohls_one(&s, id).await?);
        }
        "bulk" => {
            let mut ids: Vec<u64> = vec![];
            for e in b["edits"].as_array().into_iter().flatten().take(5000) {
                let (f, id) = (text_of(&e["field"]), ival(e, "id") as u64);
                let v = kohls_clean(&f, &e["value"])?;
                kohls_set(&mut tx, id, &f, &v, &u.name).await?;
                ids.push(id);
            }
            for values in b["new"].as_array().into_iter().flatten().take(5000) {
                ids.push(kohls_add(&mut tx, &u.name, values).await?);
            }
            tx.commit().await?;
            let mut seen = HashSet::new();
            let mut rows = vec![];
            for id in ids.into_iter().filter(|i| seen.insert(*i)) {
                if let Ok(r) = kohls_one(&s, id).await {
                    rows.push(r);
                }
            }
            out.insert("rows".into(), json!(rows));
        }
        "delete" => {
            sqlx::query("DELETE FROM kohls WHERE id = ?").bind(ival(&b, "id") as u64).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        _ => return Err(AppError::NotFound),
    }
    s.bump();
    out.insert("v".into(), json!(s.version_now()));
    Ok(Json(Value::Object(out)))
}

// ================================================================ Returns

const SOURCE_FIELDS: [&str; 15] = ["store", "po", "product", "qty", "customer", "address", "return_date", "tracking", "status_cx", "label_link", "due_date", "notes", "status_wh", "refunded", "reason"];
const DATE_FIELDS: [&str; 3] = ["return_date", "due_date", "processed_at"];
const INTERVALS: [i64; 10] = [0, 5, 10, 15, 30, 60, 180, 360, 720, 1440];

/// Every column of a return as text (the order matters: RET_KEYS names them).
const RET_SELECT: &str = "SELECT id, CAST(rid AS CHAR), CAST(return_key AS CHAR), CAST(store AS CHAR), CAST(po AS CHAR), CAST(IFNULL(product,'') AS CHAR), IFNULL(CAST(qty AS CHAR),''), CAST(customer AS CHAR), \
     CAST(IFNULL(address,'') AS CHAR), IFNULL(DATE_FORMAT(return_date, '%Y-%m-%d'),''), CAST(tracking AS CHAR), CAST(status_cx AS CHAR), CAST(IFNULL(label_link,'') AS CHAR), IFNULL(DATE_FORMAT(due_date, '%Y-%m-%d'),''), \
     CAST(IFNULL(notes,'') AS CHAR), CAST(status_wh AS CHAR), CAST(refunded AS CHAR), CAST(reason AS CHAR), IF(shipped = 1, 'TRUE', 'FALSE'), IFNULL(DATE_FORMAT(processed_at, '%Y-%m-%d %H:%i'),''), \
     CAST(processed_by AS CHAR), CAST(IFNULL(wh_notes,'') AS CHAR), removed, DATE_FORMAT(first_seen, '%Y-%m-%d %H:%i'), DATE_FORMAT(last_seen, '%Y-%m-%d %H:%i'), CAST(IFNULL(ovr,'') AS CHAR) FROM returns";
const RET_KEYS: [&str; 26] = [
    "id", "rid", "key", "store", "po", "product", "qty", "customer", "address", "return_date", "tracking", "status_cx", "label_link", "due_date", "notes", "status_wh", "refunded", "reason", "shipped",
    "processed_at", "processed_by", "wh_notes", "removed", "first_seen", "last_seen", "ovr",
];

/// A return from the database as text values by name.
fn ret_map(r: &sqlx::mysql::MySqlRow) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(r.try_get::<u64, _>(0).unwrap_or(0)));
    for (i, k) in RET_KEYS.iter().enumerate().skip(1) {
        if *k == "removed" {
            m.insert((*k).into(), json!(r.try_get::<i8, _>(i).unwrap_or(0)));
        } else {
            m.insert((*k).into(), json!(r.try_get_unchecked::<String, _>(i).unwrap_or_default()));
        }
    }
    m
}

/// A return as the page sees it: the master sheet's values with what people typed over them on top.
fn public_row(mut d: Map<String, Value>) -> Map<String, Value> {
    d.remove("key");
    let ovr: Map<String, Value> = d.remove("ovr").and_then(|o| serde_json::from_str(o.as_str().unwrap_or("")).ok()).unwrap_or_default();
    let overridden: Map<String, Value> = ovr.keys().map(|f| (f.clone(), d.get(f).cloned().unwrap_or(json!("")))).collect(); // the master sheet's value of each cell that was typed over
    d.extend(ovr);
    d.insert("overridden".into(), Value::Object(overridden));
    d
}

fn words(s: &str) -> HashSet<String> {
    Regex::new(r"[a-z0-9]{3,}").unwrap().find_iter(&s.to_lowercase()).map(|m| m.as_str().to_string()).collect()
}

async fn return_settings(s: &AppState) -> AppResult<Map<String, Value>> {
    let mut out: Map<String, Value> = serde_json::from_value(json!({
        "source_url": "https://docs.google.com/spreadsheets/d/1aBeg0dhwkBGBvC5H3CbTP_LUXlmBUDo6xiGfxn0mKh0/edit", "source_tab": "RETURNS", "warehouse": "FLORIDA", "interval_min": 30,
        "seed_url": "https://docs.google.com/spreadsheets/d/1M_1k_91nymXWt0SDt3aS5ZCaC8k3rr-hdos1qQee97M/edit", "seed_tab": "RETURNS",
    }))?;
    if let Some(Value::Object(m)) = settings::get(&s.db, "returns.settings").await? {
        out.extend(m);
    }
    Ok(out)
}

async fn sync_row(s: &AppState, sql: &str) -> AppResult<Option<Value>> {
    let Some(r) = sqlx::query(sql).fetch_optional(&s.db).await? else { return Ok(None) };
    let g = |i| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
    Ok(Some(json!({
        "id": r.try_get::<u64, _>(0).unwrap_or(0), "at": g(1), "ok": r.try_get::<i8, _>(2).unwrap_or(0), "trigger": g(3), "message": g(4),
        "read": r.try_get::<i32, _>(5).unwrap_or(0), "added": r.try_get::<i32, _>(6).unwrap_or(0), "updated": r.try_get::<i32, _>(7).unwrap_or(0), "removed": r.try_get::<i32, _>(8).unwrap_or(0),
    })))
}

const LOG_SELECT: &str = "SELECT id, DATE_FORMAT(at, '%Y-%m-%d %H:%i'), ok, CAST(trigger_by AS CHAR), CAST(IFNULL(message,'') AS CHAR), read_count, added, updated, removed FROM returns_sync_log";

async fn status(s: &AppState) -> AppResult<Value> {
    let cfg = return_settings(s).await?;
    let last = sync_row(s, &format!("{LOG_SELECT} ORDER BY id DESC LIMIT 1")).await?;
    let good = sync_row(s, &format!("{LOG_SELECT} WHERE ok = 1 ORDER BY id DESC LIMIT 1")).await?;
    let rows = sqlx::query(&format!("{LOG_SELECT} ORDER BY id DESC LIMIT 12")).fetch_all(&s.db).await?;
    let log: Vec<Value> = rows
        .iter()
        .map(|r| {
            let g = |i| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
            json!({ "id": r.try_get::<u64, _>(0).unwrap_or(0), "at": g(1), "ok": r.try_get::<i8, _>(2).unwrap_or(0), "trigger": g(3), "message": g(4),
                    "read": r.try_get::<i32, _>(5).unwrap_or(0), "added": r.try_get::<i32, _>(6).unwrap_or(0), "updated": r.try_get::<i32, _>(7).unwrap_or(0), "removed": r.try_get::<i32, _>(8).unwrap_or(0) })
        })
        .collect();
    Ok(json!({ "settings": cfg, "last": last, "last_ok": good, "log": log }))
}

/// The product picture of each return, found among the orders by P.O NUMBER (Store Order No.).
async fn images(s: &AppState, rows: &[Map<String, Value>]) -> AppResult<HashMap<u64, String>> {
    let sch = s.schema();
    let (po_c, name_c, img_c) = (sch.c("Store Order No."), sch.c("Product Name"), sch.c("Product Image Link"));
    let mut pos: Vec<String> = rows.iter().filter_map(|r| r["po"].as_str().filter(|p| !p.is_empty()).map(str::to_string)).collect();
    pos.sort();
    pos.dedup();
    let mut found: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for chunk in pos.chunks(400) {
        let sql = format!("SELECT {po_c}, {name_c}, {img_c} FROM orders_text WHERE {po_c} IN ({}) AND {img_c} <> ''", vec!["?"; chunk.len()].join(","));
        let mut q = sqlx::query(&sql);
        for p in chunk {
            q = q.bind(p);
        }
        for r in q.fetch_all(&s.db).await? {
            let g = |i| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
            found.entry(g(0)).or_default().push((g(1), g(2)));
        }
    }
    let mut out = HashMap::new();
    for r in rows {
        if let Some(cands) = r["po"].as_str().and_then(|p| found.get(p)) {
            let mine = words(r["product"].as_str().unwrap_or(""));
            // the picture of the product whose name shares the most words; the first one wins a tie (as in the Python app)
            let mut best: Option<(usize, &String)> = None;
            for c in cands {
                let score = mine.intersection(&words(&c.0)).count();
                if best.is_none_or(|(b, _)| score > b) {
                    best = Some((score, &c.1));
                }
            }
            if let Some((_, img)) = best {
                out.insert(r["id"].as_u64().unwrap_or(0), img.clone());
            }
        }
    }
    Ok(out)
}

pub async fn returns_get(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query(&format!("{RET_SELECT} ORDER BY id")).fetch_all(&s.db).await?;
    let mut rows: Vec<Map<String, Value>> = rows.iter().map(|r| public_row(ret_map(r))).collect();
    let imgs = images(&s, &rows).await?;
    for r in rows.iter_mut() {
        let id = r["id"].as_u64().unwrap_or(0);
        r.insert("image".into(), json!(imgs.get(&id).cloned().unwrap_or_default()));
    }
    let stored = settings::get(&s.db, "locks").await?;
    let cols = stored.as_ref().and_then(|l| l.pointer("/returns/cols")).cloned().unwrap_or(json!({}));
    let mut row_locks = Map::new();
    for (lid, rids) in stored.as_ref().and_then(|l| l.pointer("/returns/rows")).and_then(Value::as_object).into_iter().flatten() {
        for rid in rids.as_array().into_iter().flatten() {
            row_locks.insert(text_of(rid), json!(lid));
        }
    }
    let lk = json!({ "defs": stored.as_ref().and_then(|l| l.get("defs")).cloned().unwrap_or(json!({})), "cols": cols, "rows": row_locks });
    let labels = settings::get(&s.db, "return_labels").await?.unwrap_or(json!({}));
    Ok(Json(json!({ "rows": rows, "status": status(&s).await?, "locks": lk, "labels": labels })))
}

fn norm_when(s: &str) -> String {
    let s = s.trim();
    for (fmt, time) in [("%B %d, %Y", false), ("%b %d, %Y", false), ("%Y-%m-%d", false), ("%m/%d/%Y", false), ("%Y-%m-%d %H:%M", true), ("%Y-%m-%d %H:%M:%S", true), ("%m/%d/%Y %H:%M:%S", true), ("%m/%d/%Y %H:%M", true)] {
        if time {
            if let Ok(d) = NaiveDateTime::parse_from_str(s, fmt) {
                return d.format("%Y-%m-%d %H:%M").to_string();
            }
        } else if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return d.format("%Y-%m-%d").to_string();
        }
    }
    s.to_string()
}

fn clean_value(field: &str, v: &Value) -> AppResult<String> {
    let mut v = text_of(v).trim().to_string();
    if DATE_FIELDS.contains(&field) {
        v = norm_when(&v);
        if !v.is_empty() && !Regex::new(r"^\d{4}-\d{2}-\d{2}( \d{2}:\d{2})?$").unwrap().is_match(&v) {
            return Err(AppError::bad("Use a date like Sep 23, 2026."));
        }
    } else if field == "qty" {
        if !v.is_empty() && !v.chars().all(|c| c.is_ascii_digit()) {
            return Err(AppError::bad("QTY takes a whole number."));
        }
    } else if field == "refunded" {
        match v.to_lowercase().as_str() {
            "true" | "yes" | "y" | "1" | "x" | "checked" => v = "TRUE".into(),
            "false" | "no" | "n" | "0" | "" => v = "FALSE".into(),
            _ => return Err(AppError::bad("A checkbox takes TRUE or FALSE.")),
        }
    }
    Ok(v.chars().take(2000).collect())
}

/// Writes one text value to the typed column of a return.
fn bind_field<'q>(q: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>, field: &str, v: &str) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match field {
        "qty" => q.bind(num(v).map(|f| f.round() as i64)),
        "return_date" | "due_date" => q.bind(v.get(..10).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())),
        "processed_at" => q.bind(NaiveDateTime::parse_from_str(v, "%Y-%m-%d %H:%M").ok().or_else(|| v.get(..10).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).and_then(|d| d.and_hms_opt(0, 0, 0)))),
        "shipped" => q.bind((v == "TRUE") as i64),
        other => {
            let _ = other;
            q.bind(v.to_string())
        }
    }
}

fn returns_blocked(locks: &Locks, actor: &User, field: &str, rid: &str) -> String {
    locks.blocked_returns(actor, field, rid)
}

pub async fn returns_post(CurrentUser(u): CurrentUser, State(s): State<AppState>, Path(action): Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let admin_only = ["labels", "sync", "settings"];
    if admin_only.contains(&action.as_str()) && !u.is_admin() {
        return Err(AppError::Forbidden("Only admins can do this.".into()));
    }
    match action.as_str() {
        "labels" => {
            let field = text_of(&b["k"]);
            if !SOURCE_FIELDS.contains(&field.as_str()) && !["rid", "image", "shipped", "processed_at", "processed_by", "wh_notes"].contains(&field.as_str()) {
                return Err(AppError::bad("That column does not exist."));
            }
            let label = text_of(&b["label"]).split_whitespace().collect::<Vec<_>>().join(" ");
            if label.chars().count() > 60 {
                return Err(AppError::bad("The name can be up to 60 characters."));
            }
            let mut labels = settings::get(&s.db, "return_labels").await?.unwrap_or(json!({}));
            if label.is_empty() {
                labels.as_object_mut().map(|m| m.remove(&field));
            } else {
                labels[&field] = json!(label);
            }
            settings::put(&s.db, "return_labels", &labels).await?;
            s.bump();
            Ok(Json(json!({ "labels": labels })))
        }
        "sync" => {
            let result = sync(&s, "manual").await?;
            let mut out = result.as_object().cloned().unwrap_or_default();
            out.insert("status".into(), status(&s).await?);
            Ok(Json(Value::Object(out)))
        }
        "settings" => {
            let cur = return_settings(&s).await?;
            let mut new = cur.clone();
            let re = Regex::new(r"/spreadsheets/d/([A-Za-z0-9_-]{20,})").unwrap();
            for k in ["source_url", "seed_url"] {
                let v = b.get(k).map(text_of).unwrap_or_else(|| text_of(&cur[k])).trim().to_string();
                if !v.is_empty() && !re.is_match(&v) {
                    return Err(AppError::bad("That is not a Google Sheets link. It should look like https://docs.google.com/spreadsheets/d/…"));
                }
                new.insert(k.into(), json!(v));
            }
            for k in ["source_tab", "seed_tab", "warehouse"] {
                let v = clip(&b.get(k).map(text_of).unwrap_or_else(|| text_of(&cur[k])), 60);
                if v.is_empty() {
                    return Err(AppError::bad("Tab name and warehouse name cannot be empty."));
                }
                new.insert(k.into(), json!(v));
            }
            let interval = b.get("interval_min").map(|v| ival(&json!({ "x": v }), "x")).unwrap_or_else(|| cur["interval_min"].as_i64().unwrap_or(30));
            if !INTERVALS.contains(&interval) {
                return Err(AppError::bad("Pick one of the listed sync intervals."));
            }
            new.insert("interval_min".into(), json!(interval));
            settings::put(&s.db, "returns.settings", &Value::Object(new)).await?;
            Ok(Json(json!({ "status": status(&s).await? })))
        }
        "edit" => {
            let (rid, field) = (text_of(&b["rid"]), text_of(&b["field"]));
            let locks = Locks::load(&s).await?;
            let reason = returns_blocked(&locks, &u, &field, &rid);
            if !reason.is_empty() {
                return Err(AppError::bad(reason));
            }
            let row = edit_return(&s, &u, &rid, &field, &b["value"], b.get("revert").and_then(Value::as_bool).unwrap_or(false)).await?;
            s.bump();
            Ok(Json(json!({ "row": row, "v": s.version_now() })))
        }
        _ => Err(AppError::NotFound),
    }
}

async fn edit_return(s: &AppState, u: &User, rid: &str, field: &str, value: &Value, revert: bool) -> AppResult<Value> {
    let Some(r) = sqlx::query(&format!("{RET_SELECT} WHERE rid = ?")).bind(rid).fetch_optional(&s.db).await? else { return Err(AppError::bad("That return no longer exists.")) };
    let row = ret_map(&r);
    let who = clip(&u.name, 60);
    match field {
        "shipped" => {
            let on = match value {
                Value::Bool(b) => *b,
                Value::String(x) => !x.is_empty() && x != "false" && x != "FALSE" && x != "0",
                Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
                _ => false,
            };
            if on && who.is_empty() {
                return Err(AppError::bad("Enter your name first, so it is recorded who processed this return."));
            }
            sqlx::query("UPDATE returns SET shipped = ?, processed_at = ?, processed_by = ? WHERE rid = ?").bind(on as i64).bind(on.then(now_minute)).bind(if on { who } else { String::new() }).bind(rid).execute(&s.db).await?;
        }
        "wh_notes" => {
            sqlx::query("UPDATE returns SET wh_notes = ? WHERE rid = ?").bind(clip(&text_of(value), 2000)).bind(rid).execute(&s.db).await?;
        }
        "processed_at" | "processed_by" => {
            let v = clean_value(field, value)?;
            let v = if field == "processed_by" { v.chars().take(60).collect() } else { v };
            bind_field(sqlx::query(&format!("UPDATE returns SET {field} = ? WHERE rid = ?")), field, &v).bind(rid).execute(&s.db).await?;
        }
        f if SOURCE_FIELDS.contains(&f) => {
            let mut ovr: Map<String, Value> = row["ovr"].as_str().and_then(|o| serde_json::from_str(o).ok()).unwrap_or_default();
            if revert {
                ovr.remove(f);
            } else {
                let v = clean_value(f, value)?;
                if v == row[f].as_str().unwrap_or("") {
                    ovr.remove(f); // typed back to the master sheet's value: nothing left to remember
                } else {
                    ovr.insert(f.into(), json!(v));
                }
            }
            sqlx::query("UPDATE returns SET ovr = ? WHERE rid = ?").bind(if ovr.is_empty() { None } else { Some(Value::Object(ovr).to_string()) }).bind(rid).execute(&s.db).await?;
        }
        _ => return Err(AppError::bad("That column cannot be edited.")),
    }
    let r = sqlx::query(&format!("{RET_SELECT} WHERE rid = ?")).bind(rid).fetch_one(&s.db).await?;
    Ok(Value::Object(public_row(ret_map(&r))))
}

// ---------------------------------------------------------------- reading the master sheet

fn field_of(header: &str) -> Option<&'static str> {
    let h = header.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    Some(match h.as_str() {
        "return id" => "rid",
        "store" => "store",
        x if x.starts_with("p.o") => "po",
        "product name" => "product",
        "qty" => "qty",
        "customer name" => "customer",
        "address" => "address",
        "return date" => "return_date",
        x if x.starts_with("tracking") => "tracking",
        "return status" => "status_cx",
        x if x.starts_with("return status from cx") => "status_cx",
        "return label link" => "label_link",
        "return due date" => "due_date",
        "notes" => "notes",
        "status" => "status_wh",
        x if x.starts_with("return status from warehouse") => "status_wh",
        x if x.starts_with("source refund") => "refunded",
        "reason" => "reason",
        "shipped to amazon" => "shipped",
        x if x.starts_with("processed date") => "processed_at",
        x if x.starts_with("processed by") => "processed_by",
        _ => return None,
    })
}

async fn fetch_csv(url: &str, tab: &str) -> AppResult<Vec<Vec<String>>> {
    let re = Regex::new(r"/spreadsheets/d/([A-Za-z0-9_-]{20,})").unwrap();
    let Some(m) = re.captures(url) else { return Err(AppError::bad("The sheet link is not a Google Sheets link.")) };
    let client = reqwest::Client::builder().timeout(Duration::from_secs(90)).user_agent("Florida3PL").build()?;
    let resp = client
        .get(format!("https://docs.google.com/spreadsheets/d/{}/gviz/tq", &m[1]))
        .query(&[("tqx", "out:csv"), ("sheet", tab)])
        .send()
        .await
        .map_err(|e| AppError::bad(format!("Could not reach Google Sheets ({e}).")))?;
    if !resp.status().is_success() {
        return Err(AppError::bad(format!("Google refused to share the sheet (error {}). It must be shared as \"Anyone with the link can view\".", resp.status().as_u16())));
    }
    let body = resp.text().await.map_err(|e| AppError::bad(format!("Could not reach Google Sheets ({e}).")))?;
    let body = body.trim_start_matches('\u{feff}');
    if body.trim_start().starts_with('<') {
        return Err(AppError::bad("Google did not return the sheet. It must be shared as \"Anyone with the link can view\"."));
    }
    let mut rd = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(body.as_bytes());
    Ok(rd.records().filter_map(Result::ok).map(|r| r.iter().map(str::to_string).collect()).collect())
}

type Record = HashMap<&'static str, String>;

/// CSV rows -> records by field. With `warehouse`, only the rows whose first column holds that name (the master sheet).
fn records(rows: &[Vec<String>], warehouse: Option<&str>) -> AppResult<Vec<Record>> {
    let Some(head) = rows.first() else { return Err(AppError::bad("The sheet is empty.")) };
    let fields: Vec<Option<&'static str>> = head.iter().map(|h| field_of(h)).collect();
    if !fields.contains(&Some("po")) || !fields.contains(&Some("product")) {
        return Err(AppError::bad("That tab does not look like a returns tab (no P.O NUMBER / PRODUCT NAME columns). Check the tab name."));
    }
    let mut out = vec![];
    for r in &rows[1..] {
        if let Some(w) = warehouse {
            if r.first().is_none_or(|c| c.trim().to_uppercase() != w.trim().to_uppercase()) {
                continue;
            }
        }
        let mut rec: Record = HashMap::new();
        for (i, f) in fields.iter().enumerate() {
            if let Some(f) = f {
                rec.insert(f, r.get(i).map(|c| c.trim().to_string()).unwrap_or_default());
            }
        }
        let has = |k: &str| rec.get(k).is_some_and(|v| !v.is_empty());
        if !(has("po") || has("tracking") || has("product")) {
            continue;
        }
        for f in DATE_FIELDS {
            if let Some(v) = rec.get_mut(f).filter(|v| !v.is_empty()) {
                *v = norm_when(v);
            }
        }
        for f in ["refunded", "shipped"] {
            if let Some(v) = rec.get_mut(f) {
                *v = if v.trim().to_uppercase() == "TRUE" { "TRUE".into() } else { "FALSE".into() };
            }
        }
        out.push(rec);
    }
    Ok(out)
}

fn make_key(rec: &Record) -> String {
    ["po", "tracking", "product"].iter().map(|f| rec.get(f).map(|v| v.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()).collect::<Vec<_>>().join("|")
}

async fn next_rid(tx: &mut crate::orders_write::Tx<'_>) -> AppResult<String> {
    let rids: Vec<String> = sqlx::query_scalar("SELECT CAST(rid AS CHAR) FROM returns").fetch_all(&mut **tx).await?;
    let re = Regex::new(r"(\d+)$").unwrap();
    let top = rids.iter().filter_map(|r| re.captures(r).and_then(|c| c[1].parse::<u64>().ok())).max().unwrap_or(0);
    Ok(format!("STR-{:02}", top + 1))
}

async fn insert_return(tx: &mut crate::orders_write::Tx<'_>, rec: &Record, key: &str, now: NaiveDateTime) -> AppResult<()> {
    let rid = match rec.get("rid").filter(|r| !r.is_empty()) {
        Some(r) => r.clone(),
        None => next_rid(tx).await?,
    };
    let mut cols = vec!["rid", "return_key"];
    cols.extend(SOURCE_FIELDS);
    cols.extend(["shipped", "processed_at", "processed_by", "first_seen", "last_seen"]);
    let sql = format!("INSERT INTO returns ({}) VALUES ({})", cols.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(","), vec!["?"; cols.len()].join(","));
    let mut q = sqlx::query(&sql).bind(rid).bind(key);
    for f in SOURCE_FIELDS {
        q = bind_field(q, f, rec.get(f).map(String::as_str).unwrap_or(""));
    }
    q = bind_field(q, "shipped", rec.get("shipped").map(String::as_str).unwrap_or("FALSE"));
    q = bind_field(q, "processed_at", rec.get("processed_at").map(String::as_str).unwrap_or(""));
    q = q.bind(rec.get("processed_by").cloned().unwrap_or_default()).bind(now).bind(now);
    q.execute(&mut **tx).await?;
    Ok(())
}

/// Read the master sheet and bring the returns up to date. A failure is logged too.
pub async fn sync(s: &AppState, trigger: &str) -> AppResult<Value> {
    let cfg = return_settings(s).await?;
    let mut read = 0usize;
    let result = sync_inner(s, trigger, &cfg, &mut read).await;
    if let Err(AppError::Bad(msg)) = &result {
        sqlx::query("INSERT INTO returns_sync_log (at, ok, trigger_by, message, read_count) VALUES (?,?,?,?,?)").bind(now_minute()).bind(0).bind(trigger).bind(msg).bind(read as i32).execute(&s.db).await?;
    }
    result
}

async fn sync_inner(s: &AppState, trigger: &str, cfg: &Map<String, Value>, read: &mut usize) -> AppResult<Value> {
    let (mut added, mut updated, mut removed) = (0, 0, 0);
    let mut notes: Vec<String> = vec![];
    let master = records(&fetch_csv(&text_of(&cfg["source_url"]), &text_of(&cfg["source_tab"])).await?, Some(&text_of(&cfg["warehouse"])))?;
    *read = master.len();
    let first_time = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM returns").fetch_one(&s.db).await? == 0;
    let mut seed: Vec<Record> = vec![];
    if first_time && !text_of(&cfg["seed_url"]).is_empty() {
        // start from the 3PL sheet's own RETURNS tab, so Return IDs and Shipped to Amazon carry over
        match fetch_csv(&text_of(&cfg["seed_url"]), &text_of(&cfg["seed_tab"])).await.and_then(|r| records(&r, None)) {
            Ok(r) => seed = r,
            Err(AppError::Bad(e)) => notes.push(format!("Could not copy the 3PL sheet's current progress: {e}")),
            Err(e) => return Err(e),
        }
    }
    let now = now_minute();
    let mut tx = s.db.begin().await?;
    let mut seen: HashSet<String> = HashSet::new();
    for rec in &seed {
        let key = make_key(rec);
        if !seen.contains(&key) && rec.get("rid").is_some_and(|r| !r.is_empty()) {
            seen.insert(key.clone());
            insert_return(&mut tx, rec, &key, now).await?;
        }
    }
    if !seed.is_empty() {
        notes.push(format!("Copied {} returns with their progress from the 3PL sheet.", seen.len()));
    }
    let existing: HashMap<String, (u64, Map<String, Value>)> = sqlx::query(RET_SELECT).fetch_all(&mut *tx).await?.iter().map(|r| {
        let m = ret_map(r);
        (m["key"].as_str().unwrap_or("").to_string(), (m["id"].as_u64().unwrap_or(0), m))
    }).collect();
    let mut seen: HashSet<String> = HashSet::new();
    for rec in &master {
        let mut key = make_key(rec);
        let mut n = 1;
        while seen.contains(&key) {
            n += 1;
            key = format!("{}#{n}", make_key(rec));
        }
        seen.insert(key.clone());
        match existing.get(&key) {
            None => {
                insert_return(&mut tx, rec, &key, now).await?;
                added += 1;
            }
            Some((id, row)) => {
                let changed: Vec<&str> = SOURCE_FIELDS.iter().copied().filter(|f| row[*f].as_str().unwrap_or("") != rec.get(f).map(String::as_str).unwrap_or("")).collect();
                if !changed.is_empty() || row["removed"].as_i64().unwrap_or(0) != 0 {
                    let sets: Vec<String> = changed.iter().map(|f| format!("`{f}` = ?")).collect();
                    let sql = format!("UPDATE returns SET {}{}removed = 0, last_seen = ? WHERE id = ?", sets.join(", "), if sets.is_empty() { "" } else { ", " });
                    let mut q = sqlx::query(&sql);
                    for f in &changed {
                        q = bind_field(q, f, rec.get(f).map(String::as_str).unwrap_or(""));
                    }
                    q.bind(now).bind(id).execute(&mut *tx).await?;
                    updated += 1;
                } else {
                    sqlx::query("UPDATE returns SET last_seen = ? WHERE id = ?").bind(now).bind(id).execute(&mut *tx).await?;
                }
            }
        }
    }
    for (key, (id, row)) in &existing {
        if !seen.contains(key) && row["removed"].as_i64().unwrap_or(0) == 0 {
            sqlx::query("UPDATE returns SET removed = 1 WHERE id = ?").bind(id).execute(&mut *tx).await?;
            removed += 1;
        }
    }
    let msg = if notes.is_empty() { "OK".to_string() } else { notes.join(" ") };
    sqlx::query("INSERT INTO returns_sync_log (at, ok, trigger_by, message, read_count, added, updated, removed) VALUES (?,?,?,?,?,?,?,?)").bind(now).bind(1).bind(trigger).bind(msg).bind(*read as i32).bind(added).bind(updated).bind(removed).execute(&mut *tx).await?;
    tx.commit().await?;
    if added > 0 || updated > 0 || removed > 0 || !seed.is_empty() {
        s.bump();
    }
    Ok(json!({ "ok": true, "at": stamp(now), "read": *read, "added": added, "updated": updated, "removed": removed, "message": notes.join(" ") }))
}

/// Runs in the background: every so often checks whether a timed sync is due.
pub fn start_scheduler(s: AppState) {
    tokio::spawn(async move {
        let mut last: Option<std::time::Instant> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let Ok(cfg) = return_settings(&s).await else { continue };
            let interval = cfg["interval_min"].as_i64().unwrap_or(0);
            if interval == 0 {
                continue;
            }
            if last.is_none() {
                if let Ok(Some(at)) = sqlx::query_scalar::<_, NaiveDateTime>("SELECT at FROM returns_sync_log ORDER BY id DESC LIMIT 1").fetch_optional(&s.db).await {
                    let ago = (clock::now() - at).num_seconds().max(0) as u64;
                    last = std::time::Instant::now().checked_sub(Duration::from_secs(ago));
                }
            }
            if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(interval as u64 * 60)) {
                last = Some(std::time::Instant::now());
                let _ = sync(&s, "auto").await; // logged; tried again at the next interval
            }
        }
    });
}

