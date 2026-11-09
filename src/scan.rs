//! The scan flow of the 3PL (the old "Tracking Number Sorter"): scan a TBA number, decide what happens to each order row that is in
//! that package, and record it.
//!
//! Flow, per order row found for the scanned number:
//!   Return to Amazon  -> print the return label, do not ship to the customer
//!   Label Voided      -> VOIDED / REFUNDED, stop
//!   Package Not Rec.  -> FLAGGED, stop and tell the PH team
//!   Label Used        -> ALREADY PROCESSED, stop
//!   otherwise count the box (Boxes Received + 1), then
//!     received >= expected  -> COMPLETE: print the label, mark shipped
//!     received <  expected  -> HOLD (set Item On Hold + Hold Date); if it has been on hold more than 24 hours: OVERDUE,
//!                              ship it now and mark the missing pieces as Package Not Received

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Json, Response};
use chrono::{NaiveDateTime, NaiveDate};
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::orders_write::{now_minute, orders_set, partial_note, with_partial_note, without_partial_note, Tx};
use crate::schema::Schema;
use crate::{clock, AppState};

const OVERDUE_HOURS: f64 = 24.0;

/// outcome -> (title, tone, what to print)
fn outcome(name: &str) -> (&'static str, &'static str, Option<&'static str>) {
    match name {
        "return" => ("RETURN TO AMAZON", "violet", Some("return")),
        "voided" => ("VOIDED / REFUNDED", "crit", None),
        "pnr" => ("FLAGGED: PACKAGE NOT RECEIVED", "crit", None),
        "processed" => ("ALREADY PROCESSED", "warn", None),
        "complete" => ("PRODUCT COMPLETE: SHIP NOW", "ok", Some("ship")),
        "hold" => ("HOLD: MORE PIECES COMING", "warn", None),
        "overdue" => ("OVERDUE HOLD WARNING", "crit", Some("ship_overdue")),
        "nolabel" => ("NO LABEL FOUND", "crit", None),
        _ => ("COUNT THE ITEMS", "info", None), // "count"
    }
}

fn drive_id() -> Regex {
    Regex::new(r"/d/([-\w]{25,})").unwrap()
}

/// One order row by header (every cell as text).
pub struct Order {
    pub rn: u64,
    v: HashMap<String, String>,
}

impl Order {
    fn g(&self, h: &str) -> &str {
        self.v.get(h).map(String::as_str).unwrap_or("")
    }
}

pub async fn get_order(tx: &mut Tx<'_>, sch: &Schema, rn: u64) -> AppResult<Option<Order>> {
    let cols: Vec<String> = (0..sch.n()).map(|i| format!("c{i}")).collect();
    let Some(row) = sqlx::query(&format!("SELECT {} FROM orders_text WHERE rn = ?", cols.join(", "))).bind(rn).fetch_optional(&mut **tx).await? else { return Ok(None) };
    Ok(Some(Order { rn, v: sch.ix.iter().map(|(h, i)| (h.clone(), row.try_get_unchecked::<String, _>(*i).unwrap_or_default())).collect() }))
}

fn tokens(k: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in k.split('|') {
        let t = t.trim().to_uppercase();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

fn to_int(v: &str, default: i64) -> i64 {
    v.replace(',', "").trim().parse::<f64>().map_or(default, |f| f as i64)
}

fn parse_stamp(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    s.get(..16).and_then(|t| NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M").ok()).or_else(|| s.get(..10).and_then(|t| NaiveDate::parse_from_str(t, "%Y-%m-%d").ok()).and_then(|d| d.and_hms_opt(0, 0, 0)))
}

fn stamp_now() -> String {
    now_minute().format("%Y-%m-%d %H:%M").to_string()
}

async fn log(tx: &mut Tx<'_>, operator: &str, code: &str, rn: Option<u64>, outcome: &str, note: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO scans (at, operator, code, rn, outcome, note) VALUES (?,?,?,?,?,?)").bind(now_minute()).bind(operator).bind(code).bind(rn).bind(outcome).bind(note).execute(&mut **tx).await?;
    Ok(())
}

fn describe(d: &Order) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in [
        ("rn", json!(d.rn)), ("label", json!(d.g("Label Number"))), ("product", json!(d.g("Product Name"))), ("brand", json!(d.g("Brand"))), ("qty", json!(d.g("QTY"))),
        ("customer", json!(d.g("Customer Name"))), ("store_order", json!(d.g("Store Order No."))), ("source_order", json!(d.g("Source Order Number"))),
        ("image", json!(d.g("Product Image Link"))), ("has_label", json!(drive_id().is_match(d.g("Shipping Label Link")))), ("store", json!(d.g("Store"))), ("notes", json!(d.g("Notes"))),
    ] {
        m.insert(k.into(), v);
    }
    m
}

fn result_for(d: &Order, outcome_name: &str, message: &str, extra: Value) -> Value {
    let (title, tone, prints) = outcome(outcome_name);
    let mut m = describe(d);
    m.insert("outcome".into(), json!(outcome_name));
    m.insert("title".into(), json!(title));
    m.insert("tone".into(), json!(tone));
    m.insert("message".into(), json!(message));
    m.insert("print".into(), json!(prints));
    if let Some(e) = extra.as_object() {
        m.extend(e.clone());
    }
    Value::Object(m)
}

fn hold_hours(d: &Order) -> Option<f64> {
    if d.g("ITEM ON HOLD") != "TRUE" || d.g("Hold Date").is_empty() {
        return None;
    }
    let since = parse_stamp(d.g("Hold Date"))?;
    Some((clock::now() - since).num_seconds() as f64 / 3600.0)
}

/// Other items of the same customer order that are waiting on a hold (the sheet's "Hold Warning").
async fn siblings_on_hold(tx: &mut Tx<'_>, sch: &Schema, d: &Order) -> AppResult<Value> {
    if d.g("Store Order No.").is_empty() {
        return Ok(json!([]));
    }
    let sql = format!("SELECT {}, {} FROM orders_text WHERE {} = ? AND rn <> ? AND {} = 'TRUE'", sch.c("Label Number"), sch.c("Product Name"), sch.c("Store Order No."), sch.c("ITEM ON HOLD"));
    let rows = sqlx::query(&sql).bind(d.g("Store Order No.")).bind(d.rn).fetch_all(&mut **tx).await?;
    Ok(json!(rows.iter().map(|r| json!({ "label": r.try_get_unchecked::<String, _>(0).unwrap_or_default(), "product": r.try_get_unchecked::<String, _>(1).unwrap_or_default() })).collect::<Vec<_>>()))
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Decide the outcome for one order row and apply the box count / hold changes. A row with QTY above 1 first asks the operator how many
/// items are in the package (outcome "count"); the answer comes back as `items`, is added to Items Received, and the label is held
/// until Items Received reaches QTY.
async fn evaluate(tx: &mut Tx<'_>, sch: &Schema, who: &str, d: &Order, code: &str, dry: bool, items: Option<i64>) -> AppResult<Value> {
    if d.g("Tracking Status") == "Return to Amazon" {
        let note = if d.g("Shipped to Amazon") == "TRUE" { "It was already marked shipped to Amazon before." } else { "" };
        return Ok(result_for(d, "return", &format!("Print the return label. Do not ship to the customer. Put it in the returns area. It is marked shipped to Amazon once the label prints. {note}"), json!({})));
    }
    if d.g("Label Voided") == "TRUE" {
        return Ok(result_for(d, "voided", "Do not process or ship this item.", json!({})));
    }
    if d.g("Package Not Received") == "TRUE" {
        return Ok(result_for(d, "pnr", "Stop. Notify the PH team.", json!({})));
    }
    if d.g("Label Used") == "TRUE" {
        return Ok(result_for(d, "processed", "The label was already printed for this item. Stop.", json!({})));
    }

    let listed = tokens(d.g("Source Tracking Number"));
    let expected = if listed.is_empty() { to_int(d.g("Boxes Expected"), 1).max(1) } else { listed.len() as i64 };
    let mut received = to_int(d.g("Boxes Received"), 0);
    let seen = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM boxes WHERE rn = ? AND code = ?").bind(d.rn).bind(code).fetch_one(&mut **tx).await? > 0;
    let qty = to_int(d.g("QTY"), 1).max(1);
    let have_items = to_int(d.g("Items Received"), 0);
    if qty > 1 && !seen && items.is_none() && have_items < qty {
        return Ok(result_for(
            d,
            "count",
            &format!("This order is for {qty} items. How many are in this package? Count them and confirm."),
            json!({ "boxes": { "received": received + 1, "expected": expected, "remaining": (expected - received - 1).max(0) }, "qty_declared": qty, "items_received": have_items }),
        ));
    }
    let mut writes: Vec<(&str, String)> = vec![];
    let mut total_items = have_items;
    if let (Some(n), false) = (items, seen) {
        total_items = have_items + n.max(0);
        writes.push(("Items Received", total_items.to_string()));
    } else if qty == 1 && received + if seen { 0 } else { 1 } >= expected && have_items < 1 {
        total_items = 1;
        writes.push(("Items Received", "1".into()));
    }
    if !seen {
        received += 1;
        writes.push(("Boxes Received", received.to_string()));
        if !dry {
            sqlx::query("INSERT INTO boxes (rn, code, at) VALUES (?,?,?)").bind(d.rn).bind(code).bind(now_minute()).execute(&mut **tx).await?;
        }
    }
    if d.g("Boxes Expected") != expected.to_string() {
        writes.push(("Boxes Expected", expected.to_string()));
    }
    let boxes = json!({ "received": received, "expected": expected, "remaining": (expected - received).max(0) });
    let short = qty > 1 && total_items < qty; // fewer items than the order's QTY have arrived so far
    let extra = json!({ "boxes": boxes, "duplicate": seen, "items": { "received": total_items, "qty": qty } });
    let with = |more: Value| -> Value {
        let mut e = extra.as_object().cloned().unwrap_or_default();
        e.extend(more.as_object().cloned().unwrap_or_default());
        Value::Object(e)
    };
    let note = format!("Scan {code}");
    if received >= expected && !short {
        if !drive_id().is_match(d.g("Shipping Label Link")) {
            if !dry {
                orders_set(tx, sch, who, d.rn, &writes, &note).await?;
            }
            return Ok(result_for(d, "nolabel", "This order has no shipping label link. Tell the PH team.", extra));
        }
        if !dry {
            orders_set(tx, sch, who, d.rn, &writes, &note).await?;
        }
        let warn = siblings_on_hold(tx, sch, d).await?;
        return Ok(result_for(d, "complete", "All boxes are here. The label prints and the item is marked shipped.", with(json!({ "siblings": warn }))));
    }

    if let Some(hours) = hold_hours(d).filter(|h| *h > OVERDUE_HOURS) {
        if !dry {
            orders_set(tx, sch, who, d.rn, &writes, &note).await?;
        }
        return Ok(result_for(d, "overdue", &format!("On hold for {} hours. Ship it now. The missing pieces will be marked Package Not Received.", hours as i64), with(json!({ "hold_hours": r1(hours) }))));
    }
    writes.push(("ITEM ON HOLD", "TRUE".into()));
    if d.g("Hold Date").is_empty() || d.g("ITEM ON HOLD") != "TRUE" {
        writes.push(("Hold Date", stamp_now()));
    }
    if short {
        // leave a note in Notes: how many items have arrived so far
        writes.push(("Notes", with_partial_note(d.g("Notes"), total_items, qty)));
    }
    if !dry {
        orders_set(tx, sch, who, d.rn, &writes, &note).await?;
    }
    let left = boxes["remaining"].as_i64().unwrap_or(0);
    let msg = if short {
        format!("{total_items} of {qty} items received. Hold the label until the rest of the quantity is scanned. Put the item on the hold shelf.")
    } else {
        format!("{left} more box{} expected. Put the item on the hold shelf.", if left != 1 { "es" } else { "" })
    };
    let sib = siblings_on_hold(tx, sch, d).await?;
    Ok(result_for(d, "hold", &msg, with(json!({ "siblings": sib }))))
}

// ---------------------------------------------------------------- the actions

async fn scan_code(tx: &mut Tx<'_>, sch: &Schema, who: &str, code: &str, dry: bool) -> AppResult<Value> {
    let code = code.trim().to_uppercase();
    if code.is_empty() {
        return Err(AppError::bad("Scan or type a tracking number."));
    }
    let like: String = format!("%{}%", code.to_lowercase().chars().flat_map(|c| if matches!(c, '%' | '_' | '\\') { vec!['\\', c] } else { vec![c] }).collect::<String>());
    let rns: Vec<u64> = sqlx::query_scalar("SELECT rn FROM orders_text WHERE _s LIKE ? ORDER BY rn").bind(like).fetch_all(&mut **tx).await?;
    let mut found = vec![];
    for rn in rns {
        let Some(d) = get_order(tx, sch, rn).await? else { continue };
        if tokens(d.g("Source Tracking Number")).contains(&code) || d.g("Store Order No.").to_uppercase() == code {
            found.push(d);
        }
        if found.len() >= 40 {
            break;
        }
    }
    let mut results = vec![];
    for d in &found {
        let res = evaluate(tx, sch, who, d, &code, dry, None).await?;
        if !dry {
            let msg: String = res["message"].as_str().unwrap_or("").chars().take(200).collect();
            log(tx, who, &code, Some(d.rn), res["outcome"].as_str().unwrap_or(""), &msg).await?;
        }
        results.push(res);
    }
    if found.is_empty() && !dry {
        log(tx, who, &code, None, "notfound", "").await?;
    }
    Ok(json!({ "code": code, "found": !found.is_empty(), "practice": dry, "results": results }))
}

/// The operator counted the items in the package of an order with QTY above 1: record it and decide again.
async fn count_items(tx: &mut Tx<'_>, sch: &Schema, who: &str, rn: u64, code: &str, items: i64, dry: bool) -> AppResult<Value> {
    let code = code.trim().to_uppercase();
    if items < 0 {
        return Err(AppError::bad("Enter how many items are in the package."));
    }
    if items > 9999 {
        return Err(AppError::bad("That is too many items."));
    }
    let d = get_order(tx, sch, rn).await?.ok_or_else(|| AppError::bad("That order no longer exists."))?;
    let res = evaluate(tx, sch, who, &d, &code, dry, Some(items)).await?;
    if !dry {
        let msg: String = res["message"].as_str().unwrap_or("").chars().take(180).collect();
        log(tx, who, &code, Some(rn), res["outcome"].as_str().unwrap_or(""), &format!("counted {items}: {msg}")).await?;
    }
    Ok(json!({ "code": code, "found": true, "practice": dry, "result": res }))
}

/// Ship an overdue hold from the On hold list (the same result a scan of it would give after 24 hours).
async fn release(tx: &mut Tx<'_>, sch: &Schema, rn: u64, dry: bool) -> AppResult<Value> {
    let d = get_order(tx, sch, rn).await?;
    let Some(d) = d.filter(|d| d.g("ITEM ON HOLD") == "TRUE" && d.g("Label Used") != "TRUE") else { return Err(AppError::bad("That item is not on hold any more.")) };
    let Some(hours) = hold_hours(&d).filter(|h| *h > OVERDUE_HOURS) else { return Err(AppError::bad(format!("It has not been on hold for {} hours yet.", OVERDUE_HOURS as i64))) };
    if !drive_id().is_match(d.g("Shipping Label Link")) {
        return Ok(json!({ "code": "", "found": true, "practice": dry, "results": [result_for(&d, "nolabel", "This order has no shipping label link. Tell the PH team.", json!({}))] }));
    }
    let res = result_for(&d, "overdue", &format!("On hold for {} hours. Ship it now. The missing pieces will be marked Package Not Received.", hours as i64), json!({ "hold_hours": r1(hours) }));
    Ok(json!({ "code": "", "found": true, "practice": dry, "results": [res] }))
}

/// The label went to the printer: record it.
async fn confirm(tx: &mut Tx<'_>, sch: &Schema, who: &str, rn: u64, kind: &str, dry: bool, code: &str) -> AppResult<Value> {
    let d = get_order(tx, sch, rn).await?.ok_or_else(|| AppError::bad("That order no longer exists."))?;
    let mut w: Vec<(&str, String)> = vec![];
    match kind {
        "ship" | "ship_overdue" => {
            if d.g("Label Used") == "TRUE" {
                return Ok(json!({ "already": true }));
            }
            w.extend([("Label Used", "TRUE".to_string()), ("Shipped to Customer", "TRUE".into()), ("ITEM ON HOLD", "FALSE".into()), ("Hold Date", String::new()), ("SHIPPED TO  CUSTOMER DATE", stamp_now()), ("Updated by", who.to_string())]);
            if partial_note().is_match(d.g("Notes")) {
                w.push(("Notes", without_partial_note(d.g("Notes"))));
            }
            if d.g("Shipped to Amazon") == "TRUE" {
                w.push(("Shipped to Amazon", "FALSE".into())); // the newest scan event wins; the date stays: it still counts as shipped by that operator
            }
            if kind == "ship_overdue" {
                w.push(("Package Not Received", "TRUE".into()));
            }
        }
        "return" => {
            if d.g("Shipped to Amazon") == "TRUE" {
                return Ok(json!({ "already": true }));
            }
            w.extend([("Shipped to Amazon", "TRUE".to_string()), ("SHIPPED TO  AMAZON DATE", stamp_now()), ("Updated by", who.to_string())]);
            if d.g("Shipped to Customer") == "TRUE" {
                w.push(("Shipped to Customer", "FALSE".into())); // it went back to Amazon, so the tick goes, the date stays
            }
        }
        _ => return Err(AppError::bad("Unknown print kind.")),
    }
    if !dry {
        let note = match kind {
            "ship" => "Label printed",
            "ship_overdue" => "Overdue hold shipped",
            _ => "Return label printed",
        };
        orders_set(tx, sch, who, rn, &w, note).await?;
        log(tx, who, code, Some(rn), if kind != "ship_overdue" { "printed" } else { "printed_overdue" }, kind).await?;
    }
    Ok(json!({ "ok": true }))
}

const RESTORABLE: [&str; 3] = ["Label Used", "Shipped to Customer", "SHIPPED TO  CUSTOMER DATE"];

/// The operator found a product missing from the package: tick Package Not Received on that order. If the order was already marked
/// shipped (its label printed on the scan), Label Used and Shipped to Customer are unticked again, and the old values are handed
/// back so an Undo can put them back.
async fn mark_pnr(tx: &mut Tx<'_>, sch: &Schema, who: &str, rn: u64, flag: bool, dry: bool, code: &str, restore: &Value) -> AppResult<Value> {
    let d = get_order(tx, sch, rn).await?.ok_or_else(|| AppError::bad("That order no longer exists."))?;
    let mut w: Vec<(&str, String)> = vec![("Package Not Received", if flag { "TRUE" } else { "FALSE" }.to_string()), ("Updated by", who.to_string())];
    let mut saved = Map::new();
    if flag {
        if d.g("Label Used") == "TRUE" || d.g("Shipped to Customer") == "TRUE" {
            for name in RESTORABLE {
                if !d.g(name).is_empty() {
                    saved.insert(name.to_string(), json!(d.g(name)));
                }
            }
            w.extend([("Label Used", "FALSE".to_string()), ("Shipped to Customer", "FALSE".into()), ("SHIPPED TO  CUSTOMER DATE", String::new())]);
        }
    } else if let Some(r) = restore.as_object() {
        for name in RESTORABLE {
            if let Some(v) = r.get(name).and_then(Value::as_str).filter(|v| v.len() <= 20) {
                w.push((name, v.to_string()));
            }
        }
    }
    if !dry {
        orders_set(tx, sch, who, rn, &w, if flag { "Package not received" } else { "Package not received flag removed" }).await?;
        log(tx, who, code, Some(rn), if flag { "pnr_marked" } else { "pnr_cleared" }, if saved.is_empty() { "" } else { "unshipped" }).await?;
    }
    Ok(json!({ "ok": true, "flag": flag, "unshipped": !saved.is_empty(), "restore": saved }))
}

// ---------------------------------------------------------------- handlers

fn ival(b: &Value, k: &str, default: i64) -> i64 {
    match b.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Value::String(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

pub async fn scan_post(CurrentUser(u): CurrentUser, State(s): State<AppState>, Path(action): Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    scan_action(u, s, if action.is_empty() { "scan".into() } else { action }, b).await
}

pub async fn scan_root(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    scan_action(u, s, "scan".into(), b).await
}

async fn scan_action(u: crate::auth::User, s: AppState, action: String, b: Value) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let who: String = u.name.chars().take(60).collect();
    let dry = b.get("dry").and_then(Value::as_bool).unwrap_or(false);
    let code = b.get("code").and_then(Value::as_str).unwrap_or("").to_string();
    let rn = ival(&b, "rn", 0) as u64;
    let mut tx = s.db.begin().await?;
    let out = match action.as_str() {
        "scan" => scan_code(&mut tx, &sch, &who, &code, dry).await?,
        "count" => count_items(&mut tx, &sch, &who, rn, &code, ival(&b, "items", -1), dry).await?,
        "release" => release(&mut tx, &sch, rn, dry).await?,
        "pnr" => mark_pnr(&mut tx, &sch, &who, rn, b.get("flag").and_then(Value::as_bool).unwrap_or(true), dry, &code, b.get("restore").unwrap_or(&Value::Null)).await?,
        "confirm" => confirm(&mut tx, &sch, &who, rn, b.get("kind").and_then(Value::as_str).unwrap_or(""), dry, &code).await?,
        _ => return Err(AppError::NotFound),
    };
    tx.commit().await?;
    if !dry && action != "release" {
        s.bump();
    }
    Ok(Json(out))
}

pub async fn holds(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let mut tx = s.db.begin().await?;
    let sql = format!("SELECT rn FROM orders_text WHERE {} = 'TRUE' AND {} <> 'TRUE' AND {} <> 'TRUE'", sch.c("ITEM ON HOLD"), sch.c("Label Used"), sch.c("Label Voided"));
    let rns: Vec<u64> = sqlx::query_scalar(&sql).fetch_all(&mut *tx).await?;
    let mut out: Vec<Value> = vec![];
    for rn in rns {
        let Some(d) = get_order(&mut tx, &sch, rn).await? else { continue };
        let listed = tokens(d.g("Source Tracking Number"));
        let expected = if listed.is_empty() { to_int(d.g("Boxes Expected"), 1).max(1) } else { listed.len() as i64 };
        let hours = hold_hours(&d);
        let mut m = describe(&d);
        m.extend(
            json!({
                "received": to_int(d.g("Boxes Received"), 0), "expected": expected, "tracking": d.g("Source Tracking Number"), "items": to_int(d.g("Items Received"), 0),
                "qty_total": to_int(d.g("QTY"), 1).max(1), "since": d.g("Hold Date"), "hours": hours.map(r1), "overdue": hours.is_some_and(|h| h > OVERDUE_HOURS),
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        );
        out.push(Value::Object(m));
    }
    out.sort_by(|a, b| {
        let (ha, hb) = (a["hours"].as_f64(), b["hours"].as_f64());
        ha.is_none().cmp(&hb.is_none()).then(hb.unwrap_or(0.0).partial_cmp(&ha.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
    });
    Ok(Json(json!({ "holds": out })))
}

pub async fn recent(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query_as::<_, (u64, NaiveDateTime, String, String, Option<u64>, String, Option<String>)>("SELECT id, at, operator, code, rn, outcome, CAST(note AS CHAR) FROM scans ORDER BY id DESC LIMIT 60").fetch_all(&s.db).await?;
    let scans: Vec<Value> = rows.into_iter().map(|(id, at, operator, code, rn, outcome, note)| json!({ "id": id, "at": at.format("%Y-%m-%d %H:%M").to_string(), "operator": operator, "code": code, "rn": rn, "outcome": outcome, "note": note.unwrap_or_default() })).collect();
    Ok(Json(json!({ "scans": scans })))
}

/// One real example per situation, so the flow can be tried in practice mode without a scanner.
pub async fn samples(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let c = |h: &str| sch.c(h);
    let base = format!(
        "{} <> 'TRUE' AND {} <> 'TRUE' AND {} <> 'TRUE' AND {} <> 'Return to Amazon' AND {} LIKE 'https://drive.google.com/%' AND {} = ''",
        c("Label Used"), c("Label Voided"), c("Package Not Received"), c("Tracking Status"), c("Shipping Label Link"), c("Boxes Received")
    );
    let src = c("Source Tracking Number");
    let queries = [
        ("Complete: one box, ships now", format!("{base} AND {src} LIKE 'TBA%' AND {src} NOT LIKE '%|%'")),
        ("Hold: this box is only one of several", format!("{base} AND {src} LIKE 'TBA%|%TBA%'")),
        ("Voided / refunded", format!("{} = 'TRUE' AND {} <> 'Return to Amazon' AND {src} LIKE 'TBA%'", c("Label Voided"), c("Tracking Status"))),
        ("Package not received (flagged)", format!("{} = 'TRUE' AND {} <> 'TRUE' AND {} <> 'Return to Amazon' AND {src} LIKE 'TBA%'", c("Package Not Received"), c("Label Voided"), c("Tracking Status"))),
        ("Already processed", format!("{} = 'TRUE' AND {} <> 'TRUE' AND {} <> 'TRUE' AND {} <> 'Return to Amazon' AND {src} LIKE 'TBA%'", c("Label Used"), c("Label Voided"), c("Package Not Received"), c("Tracking Status"))),
        ("Return to Amazon", format!("{} = 'Return to Amazon' AND {src} LIKE 'TBA%'", c("Tracking Status"))),
    ];
    let mut out = vec![];
    for (title, where_) in queries {
        let row = sqlx::query(&format!("SELECT {src}, {} FROM orders_text WHERE {where_} ORDER BY rn LIMIT 1", c("Product Name"))).fetch_optional(&s.db).await?;
        if let Some(r) = row {
            let code = tokens(&r.try_get_unchecked::<String, _>(0).unwrap_or_default()).into_iter().next().unwrap_or_default();
            out.push(json!({ "scenario": title, "code": code, "product": r.try_get_unchecked::<String, _>(1).unwrap_or_default() }));
        }
    }
    out.push(json!({ "scenario": "Not found", "code": "TBA000000000000", "product": "" }));
    Ok(Json(json!({ "samples": out })))
}

// ---------------------------------------------------------------- the label PDF

fn label_cache() -> &'static Mutex<HashMap<u64, Vec<u8>>> {
    static C: OnceLock<Mutex<HashMap<u64, Vec<u8>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The shipping label PDF of an order, fetched from Google Drive, so the browser can print it.
pub async fn label(_u: CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Response> {
    let rn: u64 = q.get("rn").and_then(|v| v.parse().ok()).unwrap_or(0);
    if let Some(data) = label_cache().lock().unwrap().get(&rn).cloned() {
        return Ok(pdf(data));
    }
    let sch = s.schema();
    let mut tx = s.db.begin().await?;
    let d = get_order(&mut tx, &sch, rn).await?.ok_or_else(|| AppError::bad("That order no longer exists."))?;
    drop(tx);
    Ok(pdf(drive_pdf(rn, d.g("Shipping Label Link"), "This order has no shipping label link.").await?))
}

/// The return label PDF of a Store Returns card (its RETURN LABEL LINK), fetched from Google Drive, so the browser can print it.
pub async fn return_label(_u: CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Response> {
    let rid = q.get("rid").cloned().unwrap_or_default();
    let link: Option<String> = sqlx::query_scalar("SELECT CAST(IFNULL(label_link,'') AS CHAR) FROM returns WHERE rid = ? LIMIT 1").bind(&rid).fetch_optional(&s.db).await?;
    let link = link.ok_or_else(|| AppError::bad("That return no longer exists."))?;
    // the cache key of a return: its link's hash, with the top bit set so it never meets an order's row number
    let key = { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); link.hash(&mut h); h.finish() | (1 << 63) };
    Ok(pdf(drive_pdf(key, &link, "This return has no return label link.").await?))
}

/// The PDF behind a Google Drive link (kept in a small cache under `key`).
async fn drive_pdf(key: u64, link: &str, no_link: &str) -> AppResult<Vec<u8>> {
    if let Some(data) = label_cache().lock().unwrap().get(&key).cloned() {
        return Ok(data);
    }
    let Some(m) = drive_id().captures(link) else { return Err(AppError::bad(no_link)) };
    let url = format!("https://drive.google.com/uc?export=download&id={}", &m[1]);
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).user_agent("Florida3PL").build()?;
    let data = client.get(url).send().await.map_err(|e| AppError::bad(format!("Could not download the label from Google Drive ({e}).")))?.bytes().await.map_err(|e| AppError::bad(format!("Could not download the label from Google Drive ({e}).")))?;
    if !data.starts_with(b"%PDF") || data.len() > 20 * 1024 * 1024 {
        return Err(AppError::bad("Google Drive did not return a PDF. Check that the label file is shared with 'anyone with the link'."));
    }
    let data = data.to_vec();
    {
        let mut c = label_cache().lock().unwrap();
        if c.len() > 60 {
            c.clear();
        }
        c.insert(key, data.clone());
    }
    Ok(data)
}

fn pdf(data: Vec<u8>) -> Response {
    ([(CONTENT_TYPE, "application/pdf".to_string()), (CACHE_CONTROL, "no-store".to_string())], data).into_response()
}
