//! Amazon order import: the orders file downloaded from Amazon Business fills in tracking numbers and delivery statuses.
//!
//! Each line of the file is one item of one shipment. A line is matched to an All Orders row by its Order ID (the row's Source Order
//! Number). When an Amazon order holds several items, or one item was shipped in several parcels, the line is matched by its Title
//! (the row's Product Name), so the right row gets the right tracking number.
//!
//! What it writes, per row:
//!   Source Tracking Number  the Carrier Tracking # (a row that ships in several parcels lists them separated by |)
//!   Source Order Status     from Delivery Status: Delivered / In Transit. (Unfulfilled is a row with no Amazon order number; the file never sets it.)
//!   Brand                   the product's Brand (or Manufacturer), only into an EMPTY Brand cell; also on finished rows
//!   Delivery Date           from the file's expected delivery date, when the row has none or Amazon moved the date
//!   Delivered to Warehouse Date  when the row becomes Delivered: that same date
//! Rows that are already Delivered, Refunded, For Refund or Cancelled are left exactly as they are (tracking and status), because a
//! month's file lists finished orders again. Several items of one Store Order No. that arrive on different days are Partially
//! Delivered until the last one is here; then they all become Delivered. The import is shown as a preview first; nothing is written
//! until an admin confirms it.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Json;
use chrono::NaiveDate;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Value};
use sqlx::Row;

use crate::auth::AdminUser;
use crate::error::{AppError, AppResult};
use crate::orders_write::{delivered_cleanup, orders_set, read_text, refresh_search};
use crate::AppState;

const SEP: &str = "|";
const HOLD: [&str; 3] = ["Refunded", "Cancelled", "For Refund"]; // set by people, never overwritten by the file

fn is_done(status: &str) -> bool {
    let s = status.to_lowercase();
    HOLD.iter().any(|h| h.to_lowercase() == s) || s == "delivered" // finished rows: the file never rewrites them
}

fn arrived(s: &str) -> bool {
    s == "Delivered" || s == "Partially Delivered"
}

pub struct Plan {
    pub summary: Value,
    pub changes: IndexMap<u64, Change>,
}

pub struct Change {
    pub changes: Vec<(String, String)>,
    pub label: String,
    pub order: String,
    pub title: String,
    pub was: Value,
}

/// The pages' "plans kept in memory for an hour".
pub type Plans = std::sync::Arc<std::sync::Mutex<HashMap<String, (Instant, std::sync::Arc<Plan>)>>>;

fn words(text: &str) -> HashSet<String> {
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9]+").unwrap());
    WORD.find_iter(&text.to_lowercase()).map(|m| m.as_str().to_string()).filter(|w| w.len() > 2).collect()
}

/// 0..1: how much of the shorter title's words the other title has too.
fn similar(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(b).count() as f64 / a.len().min(b.len()) as f64
}

fn to_int(v: &str) -> i64 {
    v.replace(',', "").trim().parse::<f64>().map_or(0, |f| f as i64)
}

/// Excel shows long numbers (tracking numbers) as 5.42923753189E11. Give back the digits (542923753189) when that is exact.
/// Excel keeps only 15 digits of a number, so a longer one cannot be rebuilt: it is returned unchanged.
pub fn plain_number(text: &str) -> String {
    let t = text.trim();
    // built once: this runs for every cell of an imported sheet (millions of times), and building a pattern each time took minutes
    static SCI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(-?)(\d+)(?:\.(\d+))?[eE]\+?(\d+)$").unwrap());
    let Some(c) = SCI.captures(t) else { return text.to_string() };
    let (int_part, frac, exp) = (&c[2], c.get(3).map_or("", |m| m.as_str()), c[4].parse::<usize>().unwrap_or(0));
    if frac.len() > exp {
        return text.to_string(); // not a whole number
    }
    let digits = format!("{int_part}{frac}{}", "0".repeat(exp - frac.len()));
    let digits = digits.trim_start_matches('0');
    if digits.len() >= 16 {
        return text.to_string();
    }
    format!("{}{}", &c[1], if digits.is_empty() { "0" } else { digits })
}

const ETA_NAMES: [&str; 5] = ["estimated delivery", "expected delivery", "scheduled delivery", "promised delivery", "delivery date"];

/// The column of the file that holds the estimated delivery date (its header is not fixed, so look for the likely names).
fn eta_column(fieldnames: &[String]) -> Option<String> {
    let re = Regex::new(r"[^a-z]+").unwrap();
    let names: Vec<(&String, String)> = fieldnames.iter().map(|f| (f, re.replace_all(&f.to_lowercase(), " ").trim().to_string())).collect();
    for want in ETA_NAMES {
        for (f, n) in &names {
            if n.contains(want) && !n.contains("status") {
                return Some((*f).clone());
            }
        }
    }
    None
}

/// A date as written in the file -> 'YYYY-MM-DD', or "" when there is none. Takes the first date it finds (a range gives its start).
fn parse_date(text: &str) -> String {
    let t = text.trim();
    let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    static ISO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{4})-(\d{1,2})-(\d{1,2})").unwrap());
    static SLASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{1,2})/(\d{1,2})/(\d{2,4})").unwrap());
    static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([A-Za-z]{3})[a-z]*\.?\s+(\d{1,2})(?:st|nd|rd|th)?,?\s+(\d{4})").unwrap());
    let (y, mo, d);
    if let Some(m) = ISO.captures(t) {
        (y, mo, d) = (m[1].parse::<i32>().unwrap_or(0), m[2].parse::<u32>().unwrap_or(0), m[3].parse::<u32>().unwrap_or(0));
    } else if let Some(m) = SLASH.captures(t) {
        let yy = m[3].parse::<i32>().unwrap_or(0);
        (y, mo, d) = (if yy < 100 { yy + 2000 } else { yy }, m[1].parse::<u32>().unwrap_or(0), m[2].parse::<u32>().unwrap_or(0));
    } else if let Some(m) = WORDS.captures(t) {
        let Some(i) = months.iter().position(|x| *x == m[1].to_lowercase()) else { return String::new() };
        (y, mo, d) = (m[3].parse::<i32>().unwrap_or(0), i as u32 + 1, m[2].parse::<u32>().unwrap_or(0));
    } else {
        return String::new();
    }
    NaiveDate::from_ymd_opt(y, mo, d).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

#[derive(Clone)]
pub struct Line {
    pub order: String,
    pub title: String,
    pub tracking: String,
    pub delivery: String,
    pub shipment: String,
    pub eta: String,
    pub eta_column: Option<String>,
    pub brand: String,
    pub qty: i64,
}

/// The lines of the Amazon file that have an Order ID. An Err says it is not that file.
pub fn read_lines(data: &[u8]) -> AppResult<Vec<Line>> {
    let text = String::from_utf8_lossy(data);
    let text = text.trim_start_matches('\u{feff}');
    let mut rd = csv::ReaderBuilder::new().flexible(true).from_reader(text.as_bytes());
    let head: Vec<String> = rd.headers().map(|h| h.iter().map(str::to_string).collect()).unwrap_or_default();
    let need = ["Order ID", "Carrier Tracking #", "Delivery Status", "Title"];
    if head.is_empty() || !need.iter().all(|n| head.iter().any(|h| h == n)) {
        return Err(AppError::bad("This does not look like the orders file from Amazon. It needs the columns Order ID, Title, Carrier Tracking # and Delivery Status."));
    }
    let eta = eta_column(&head);
    let idx = |name: &str| head.iter().rposition(|h| h == name); // the last column of that name wins, like a dict
    let get = |r: &csv::StringRecord, name: &str| idx(name).and_then(|i| r.get(i)).unwrap_or("").to_string();
    let wrap = Regex::new(r#"^="?|"$"#).unwrap();
    let mut out = vec![];
    for r in rd.records().filter_map(Result::ok) {
        let order = get(&r, "Order ID").trim().to_string();
        if order.is_empty() {
            continue;
        }
        let tracking = plain_number(wrap.replace_all(get(&r, "Carrier Tracking #").trim(), "").trim()).to_uppercase();
        let brand = Some(get(&r, "Brand").trim().to_string()).filter(|b| !b.is_empty()).or_else(|| Some(get(&r, "Manufacturer").trim().to_string())).unwrap_or_default();
        let qty = [to_int(&get(&r, "Item Quantity")), to_int(&get(&r, "Shipment Quantity"))].into_iter().find(|q| *q != 0).unwrap_or(1);
        out.push(Line {
            order,
            title: get(&r, "Title").trim().to_string(),
            tracking,
            delivery: get(&r, "Delivery Status").trim().to_string(),
            shipment: get(&r, "Shipment Status").trim().to_string(),
            eta: eta.as_ref().map(|c| parse_date(&get(&r, c))).unwrap_or_default(),
            eta_column: eta.clone(),
            brand,
            qty,
        });
    }
    Ok(out)
}

fn line_state(l: &Line) -> &'static str {
    let d = l.delivery.to_lowercase();
    if d == "delivered" {
        "delivered"
    } else if d == "not delivered" || (l.shipment.to_lowercase() == "shipped" && !l.tracking.is_empty()) || l.shipment.to_lowercase() == "shipped" {
        "transit"
    } else {
        "unfulfilled"
    }
}

fn tokens(cell: &str, fix: bool) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in cell.split(SEP) {
        let t = t.trim();
        let t = if fix { plain_number(t) } else { t.to_string() }.to_uppercase(); // 5.42923753189E11 -> 542923753189
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// One All Orders row, as the plan needs it.
#[derive(Clone)]
pub struct OrderRow {
    pub rn: u64,
    pub order: String,
    pub tracking: String,
    pub status: String,
    pub title: String,
    pub store: String,
    pub label: String,
    pub qty: String,
    pub items: String,
    pub ddate: String,
    pub brand: String,
}

pub fn build_plan(rows: &[OrderRow], lines: &[Line]) -> Plan {
    let mut by_order: HashMap<&str, Vec<&OrderRow>> = HashMap::new();
    for r in rows.iter().filter(|r| !r.order.is_empty()) {
        by_order.entry(&r.order).or_default().push(r);
    }
    let row_words: HashMap<u64, HashSet<String>> = rows.iter().map(|r| (r.rn, words(&r.title))).collect();
    let mut lines_by_order: IndexMap<&str, Vec<&Line>> = IndexMap::new();
    for l in lines {
        lines_by_order.entry(&l.order).or_default().push(l);
    }

    let mut assigned: IndexMap<u64, Vec<&Line>> = IndexMap::new();
    let (mut unmatched, mut unsure) = (vec![], vec![]);
    for (oid, group) in &lines_by_order {
        let Some(cands) = by_order.get(oid) else {
            unmatched.push(json!({ "order": oid, "title": group[0].title, "lines": group.len() }));
            continue;
        };
        let mut taken: HashSet<u64> = HashSet::new();
        let mut sorted: Vec<&&Line> = group.iter().collect();
        sorted.sort_by_key(|l| std::cmp::Reverse(l.title.chars().count())); // stable, longest title first
        for ln in sorted {
            let w = words(&ln.title);
            let (mut best, mut best_score): (Option<&OrderRow>, f64) = (None, -1.0);
            for r in cands {
                let mut score = if cands.len() > 1 { similar(&w, &row_words[&r.rn]) } else { 1.0 };
                if !ln.tracking.is_empty() && tokens(&r.tracking, true).contains(&ln.tracking) {
                    score += 1.0;
                }
                if taken.contains(&r.rn) {
                    score -= 0.5; // another item of this order goes to a row that is still free
                }
                if score > best_score {
                    (best, best_score) = (Some(*r), score);
                }
            }
            if cands.len() > 1 && best_score <= 0.1 {
                unsure.push(json!({ "order": oid, "title": ln.title, "why": "no row of this order has a similar product name" }));
                continue;
            }
            let best = best.unwrap();
            taken.insert(best.rn);
            assigned.entry(best.rn).or_default().push(ln);
        }
    }

    let row_by_rn: HashMap<u64, &OrderRow> = rows.iter().map(|r| (r.rn, r)).collect();
    let mut changes: IndexMap<u64, Vec<(String, String)>> = IndexMap::new();
    let mut new_status: HashMap<u64, String> = HashMap::new();
    let mut est_of: HashMap<u64, String> = HashMap::new();
    let mut skipped_done = 0;
    for (rn, got) in &assigned {
        let r = row_by_rn[rn];
        let brand = got.iter().map(|l| l.brand.clone()).find(|b| !b.is_empty()).unwrap_or_default();
        let brand = if !brand.is_empty() && r.brand.trim().is_empty() { brand } else { String::new() }; // only an empty Brand cell is filled
        if is_done(&r.status) {
            if !brand.is_empty() {
                changes.insert(*rn, vec![("Brand".into(), brand)]); // a finished row: only its empty Brand is filled
            } else {
                skipped_done += 1;
            }
            continue;
        }
        let have = tokens(&r.tracking, true);
        let mut merged = have.clone();
        for l in got {
            if !l.tracking.is_empty() && !merged.contains(&l.tracking) {
                merged.push(l.tracking.clone());
            }
        }
        let states: Vec<&str> = got.iter().map(|l| line_state(l)).collect();
        // An order line can hold 2 of the same product that ship separately: the row is Delivered only when the delivered quantity covers
        // the row's QTY, otherwise it stays Partially Delivered (the file lists only what has shipped).
        let got_qty: i64 = got.iter().map(|l| l.qty).sum();
        let need = to_int(&r.qty).max(got_qty);
        let delivered_qty: i64 = got.iter().zip(&states).filter(|(_, s)| **s == "delivered").map(|(l, _)| l.qty).sum();
        let mut want = if delivered_qty >= need {
            "Delivered"
        } else if delivered_qty > 0 {
            "Partially Delivered"
        } else if states.contains(&"transit") {
            "In Transit"
        } else {
            "Unfulfilled"
        };
        // what the operator counted in the Scan tab (Items Received) beats the file for what physically arrived
        let counted = to_int(&r.items);
        if counted != 0 && (want == "Delivered" || want == "Partially Delivered") {
            want = if counted >= to_int(&r.qty).max(1) { "Delivered" } else { "Partially Delivered" };
        }
        let cur = r.status.as_str();
        let status = if HOLD.contains(&cur) {
            cur
        } else if want == "Delivered" || want == "Partially Delivered" {
            want
        } else if want == "In Transit" && ["", "Unfulfilled", "In Transit"].contains(&cur) {
            want
        } else {
            cur
        };
        // "Unfulfilled" means the row has no Amazon order number yet. A row that is in this file has one, so the file never sets it.
        let mut ch: Vec<(String, String)> = vec![];
        if !brand.is_empty() {
            ch.push(("Brand".into(), brand));
        }
        let est = got.iter().filter(|l| !l.eta.is_empty()).map(|l| l.eta.clone()).max().unwrap_or_default(); // the date Amazon now gives for this row
        est_of.insert(*rn, est.clone());
        if !est.is_empty() && r.ddate.chars().take(10).collect::<String>() != est {
            ch.push(("Delivery Date".into(), est)); // empty, or Amazon moved it
        }
        if merged != tokens(&r.tracking, false) {
            ch.push(("Source Tracking Number".into(), merged.join(SEP))); // also rewrites a cell that Excel had turned into 5.4E11
        }
        if status != cur {
            ch.push(("Source Order Status".into(), status.to_string()));
        }
        new_status.insert(*rn, status.to_string());
        if !ch.is_empty() {
            changes.insert(*rn, ch);
        }
    }

    // items of one Store Order No. that arrive on different days: Partially Delivered until the last one is here
    let mut stores: IndexMap<&str, Vec<&OrderRow>> = IndexMap::new();
    for r in rows.iter().filter(|r| !r.store.is_empty()) {
        stores.entry(&r.store).or_default().push(r);
    }
    let mut touched: Vec<&str> = vec![];
    for rn in assigned.keys() {
        let st = row_by_rn[rn].store.as_str();
        if !st.is_empty() && !touched.contains(&st) {
            touched.push(st);
        }
    }
    for store in touched {
        let status_of = |r: &OrderRow| new_status.get(&r.rn).cloned().unwrap_or_else(|| r.status.clone());
        let group: Vec<&OrderRow> = stores[store].iter().copied().filter(|r| !HOLD.contains(&status_of(r).as_str())).collect();
        if group.len() < 2 {
            continue;
        }
        let sts: Vec<(u64, String)> = group.iter().map(|r| (r.rn, status_of(r))).collect();
        let arrived_rns: Vec<u64> = sts.iter().filter(|(_, s)| arrived(s)).map(|(rn, _)| *rn).collect();
        if arrived_rns.is_empty() {
            continue;
        }
        let want = if arrived_rns.len() == group.len() { "Delivered" } else { "Partially Delivered" };
        for rn in arrived_rns {
            if is_done(&row_by_rn[&rn].status) {
                continue; // already finished: counted as arrived, never rewritten
            }
            let cur_new = sts.iter().find(|(r, _)| *r == rn).map(|(_, s)| s.clone()).unwrap_or_default();
            if cur_new != want {
                new_status.insert(rn, want.to_string());
                let cur = row_by_rn[&rn].status.as_str();
                let empty = {
                    let ch = changes.entry(rn).or_default();
                    ch.retain(|(k, _)| k != "Source Order Status");
                    if want != cur {
                        ch.push(("Source Order Status".into(), want.to_string()));
                    }
                    ch.is_empty()
                };
                if empty {
                    changes.shift_remove(&rn);
                }
            }
        }
    }

    // a row that becomes Delivered counts as arrived on the date Amazon gives, not on the day of the import
    for (rn, ch) in changes.iter_mut() {
        if ch.iter().any(|(k, v)| k == "Source Order Status" && v == "Delivered") && est_of.get(rn).is_some_and(|e| !e.is_empty()) {
            ch.push(("Delivered to Warehouse Date".into(), est_of[rn].clone()));
        }
    }

    let mut moves: IndexMap<String, usize> = IndexMap::new();
    for (rn, ch) in &changes {
        if let Some((_, v)) = ch.iter().find(|(k, _)| k == "Source Order Status") {
            let from = row_by_rn[rn].status.as_str();
            *moves.entry(format!("{} → {v}", if from.is_empty() { "(blank)" } else { from })).or_insert(0) += 1;
        }
    }
    let mut moves: Vec<(String, usize)> = moves.into_iter().collect();
    moves.sort_by_key(|m| std::cmp::Reverse(m.1));
    let count = |key: &str| changes.values().filter(|c| c.iter().any(|(k, _)| k == key)).count();
    let summary = json!({
        "lines": lines.len(), "orders": lines_by_order.len(), "matched_rows": assigned.len(), "skipped_done": skipped_done,
        "tracking_added": count("Source Tracking Number"), "status_changed": count("Source Order Status"), "dates_changed": count("Delivery Date"), "brands_added": count("Brand"),
        "eta_column": lines.iter().find_map(|l| l.eta_column.clone()), "eta_found": lines.iter().filter(|l| !l.eta.is_empty()).count(),
        "moves": moves.iter().map(|(k, n)| json!({ "move": k, "n": n })).collect::<Vec<_>>(),
        "unmatched_orders": unmatched, "unsure": unsure,
    });
    let changes = changes
        .into_iter()
        .map(|(rn, ch)| {
            let r = row_by_rn[&rn];
            (rn, Change { changes: ch, label: r.label.clone(), order: r.order.clone(), title: r.title.clone(), was: json!({ "Source Tracking Number": r.tracking, "Source Order Status": r.status, "Delivery Date": r.ddate }) })
        })
        .collect();
    Plan { summary, changes }
}

// ---------------------------------------------------------------- the web side

/// Read the Amazon orders file (raw request body) and show what importing it would change. Nothing is written yet.
pub async fn preview(_a: AdminUser, State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Value>> {
    if body.is_empty() {
        return Err(AppError::bad("The file is empty."));
    }
    let lines = read_lines(&body)?;
    if lines.is_empty() {
        return Err(AppError::bad("The file has no orders in it."));
    }
    let sch = s.schema();
    let names = ["Source Order Number", "Source Tracking Number", "Source Order Status", "Product Name", "Store Order No.", "Label Number", "QTY", "Items Received", "Delivery Date", "Brand"];
    let cols: Vec<String> = names.iter().map(|n| sch.c(n)).collect();
    let db_rows = sqlx::query(&format!("SELECT rn, {} FROM orders_text", cols.join(", "))).fetch_all(&s.db).await?;
    let rows: Vec<OrderRow> = db_rows
        .iter()
        .map(|r| {
            let t = |i: usize| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
            OrderRow { rn: r.try_get::<u64, _>(0).unwrap_or(0), order: t(1).trim().to_string(), tracking: t(2), status: t(3).trim().to_string(), title: t(4), store: t(5).trim().to_string(), label: t(6), qty: t(7), items: t(8), ddate: t(9), brand: t(10) }
        })
        .collect();
    let plan = build_plan(&rows, &lines);
    let shown: Vec<Value> = plan.changes.iter().take(300).map(|(rn, c)| json!({ "rn": rn, "changes": c.changes.iter().cloned().collect::<HashMap<_, _>>(), "label": c.label, "order": c.order, "title": c.title, "was": c.was })).collect();
    let mut out = plan.summary.as_object().cloned().unwrap_or_default();
    let unmatched: Vec<Value> = out["unmatched_orders"].as_array().cloned().unwrap_or_default();
    out.insert("unmatched_total".into(), json!(unmatched.len()));
    out.insert("unmatched_orders".into(), json!(unmatched.into_iter().take(100).collect::<Vec<_>>()));
    out.insert("shown".into(), json!(shown));
    out.insert("changed_total".into(), json!(plan.changes.len()));
    let token = {
        let mut plans = s.amazon_plans.lock().unwrap();
        plans.retain(|_, (t, _)| t.elapsed().as_secs() <= 3600);
        let mut b = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut b);
        let token = hex::encode(b);
        plans.insert(token.clone(), (Instant::now(), std::sync::Arc::new(plan)));
        token
    };
    out.insert("token".into(), json!(token));
    let file = headers.get("X-Filename").and_then(|v| v.to_str().ok()).map(|f| percent_decode(f)).unwrap_or_else(|| "file".into());
    out.insert("file".into(), json!(file));
    Ok(Json(Value::Object(out)))
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn apply(AdminUser(me): AdminUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let token = b.get("token").and_then(Value::as_str).unwrap_or("").to_string();
    let plan = s.amazon_plans.lock().unwrap().get(&token).map(|(_, p)| p.clone());
    let Some(plan) = plan else { return Err(AppError::bad("That preview has expired. Choose the file again.")) };
    let sch = s.schema();
    let mut tx = s.db.begin().await?;
    let mut skipped = 0;
    let search_idx = sch.search_idx();
    let mut touched: Vec<u64> = vec![];
    for (rn, c) in &plan.changes {
        // a last check against the live data: a finished row is never rewritten, even by an older preview
        let now_status = match sch.at("Source Order Status") {
            Some(j) => read_text(&mut tx, *rn, j).await?.unwrap_or_default(),
            None => String::new(),
        };
        let brand = c.changes.iter().find(|(k, _)| k == "Brand").map(|(_, v)| v.clone());
        if is_done(now_status.trim()) {
            if let Some(brand) = brand {
                // a finished row: only its empty Brand is filled
                let cur = match sch.at("Brand") {
                    Some(j) => read_text(&mut tx, *rn, j).await?.unwrap_or_default(),
                    None => String::new(),
                };
                if cur.trim().is_empty() {
                    orders_set(&mut tx, &sch, &me.name, *rn, &[("Brand", brand)], "Amazon import").await?;
                    touched.push(*rn);
                    continue;
                }
            }
            skipped += 1;
            continue;
        }
        let values: Vec<(&str, String)> = c.changes.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        orders_set(&mut tx, &sch, &me.name, *rn, &values, "Amazon import").await?;
        if c.changes.iter().any(|(k, v)| k == "Source Order Status" && v == "Delivered") {
            delivered_cleanup(&mut tx, &sch, &me.name, *rn, "Amazon import: row is Delivered").await?;
            touched.push(*rn);
        }
        if c.changes.iter().any(|(k, _)| sch.at(k).is_some_and(|j| search_idx.contains(&j))) {
            touched.push(*rn);
        }
    }
    touched.sort_unstable();
    touched.dedup();
    refresh_search(&mut tx, &sch, &touched).await?;
    tx.commit().await?;
    s.amazon_plans.lock().unwrap().remove(&token);
    s.bump();
    let sm = &plan.summary;
    Ok(Json(json!({ "ok": true, "rows": plan.changes.len() - skipped, "tracking_added": sm["tracking_added"], "status_changed": sm["status_changed"], "dates_changed": sm["dates_changed"], "brands_added": sm["brands_added"] })))
}
