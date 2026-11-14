//! The Dashboard: the day's numbers, the same as the Dashboard tab of the Google Sheet.
//!
//! Everything is counted from the orders for ONE day in US Eastern time, which is how every date and time in the app is recorded.
//! Operators are the columns of the hourly tables: a name for the person plus the sign-in names or e-mail addresses that mean them
//! (the "Updated by" cell). An admin edits the list; it is kept in `app_settings` (key `dashboard`), apart from the orders.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::Json;
use chrono::NaiveDate;
use serde_json::{json, Value};
use sqlx::Row;

use crate::auth::{AdminUser, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::filters::fill_expr;
use crate::{clock, settings, AppState};

struct Operator {
    label: String,
    matches: Vec<String>,
}

fn default_operators() -> Vec<Operator> {
    vec![Operator { label: "Carlos".into(), matches: vec![] }, Operator { label: "Bernardo".into(), matches: vec![] }]
}

async fn operators(s: &AppState) -> AppResult<Vec<Operator>> {
    let stored = settings::get(&s.db, "dashboard").await?;
    let Some(list) = stored.as_ref().and_then(|v| v.get("operators")).and_then(Value::as_array) else { return Ok(default_operators()) };
    Ok(list
        .iter()
        .map(|o| Operator {
            label: o.get("label").and_then(Value::as_str).unwrap_or("").to_string(),
            matches: o.get("match").and_then(Value::as_array).map(|m| m.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        })
        .collect())
}

fn operators_json(ops: &[Operator]) -> Value {
    json!(ops.iter().map(|o| json!({ "label": o.label, "match": o.matches })).collect::<Vec<_>>())
}

/// Keep at most 8 operators, each with a name and the sign-in names / e-mails that mean them.
pub async fn save_operators(_a: AdminUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let items = b.get("operators").and_then(Value::as_array).ok_or_else(|| AppError::bad("Send the list of operators."))?;
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    for it in items.iter().take(8) {
        let label: String = it.get("label").and_then(Value::as_str).unwrap_or("").trim().chars().take(40).collect();
        let matches: Vec<String> = it.get("match").and_then(Value::as_array).map(|m| m.iter().filter_map(Value::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).take(10).collect()).unwrap_or_default();
        if label.is_empty() {
            continue;
        }
        for m in &matches {
            if !seen.insert(m.to_lowercase()) {
                return Err(AppError::bad(format!("“{m}” is listed for two operators.")));
            }
        }
        out.push(Operator { label, matches });
    }
    let json = operators_json(&out);
    settings::put(&s.db, "dashboard", &json!({ "operators": json })).await?;
    Ok(Json(json!({ "operators": json })))
}

/// (hour, operator, count) rows -> 24 rows of {"total", "by": [count per operator], "other"}.
fn hours(rows: &[(String, String, i64)], ops: &[Operator]) -> Vec<Value> {
    let mut who: HashMap<String, usize> = HashMap::new();
    for (i, o) in ops.iter().enumerate() {
        for m in o.matches.iter().chain(std::iter::once(&o.label)) {
            who.insert(m.to_lowercase(), i);
        }
    }
    let mut table: Vec<(i64, Vec<i64>, i64)> = (0..24).map(|_| (0, vec![0; ops.len()], 0)).collect();
    for (hour, name, n) in rows {
        let Ok(h) = hour.parse::<usize>() else { continue };
        if h > 23 {
            continue;
        }
        table[h].0 += n;
        match who.get(&name.trim().to_lowercase()) {
            Some(i) => table[h].1[*i] += n,
            None => table[h].2 += n,
        }
    }
    table.into_iter().map(|(total, by, other)| json!({ "total": total, "by": by, "other": other })).collect()
}

fn total_of(table: &[Value]) -> i64 {
    table.iter().map(|h| h["total"].as_i64().unwrap_or(0)).sum()
}

/// (hour, operator, count) of the packages stamped on `day`. The operator is the one who did THAT shipping step (the ship record),
/// so a later scan by someone else (a return to Amazon) does not take the count away; older rows fall back to Updated by.
async fn by_hour(s: &AppState, stamp: &str, kind: &str, day: &str) -> AppResult<Vec<(String, String, i64)>> {
    let sch = s.schema();
    let (c, upd) = (sch.c(stamp), sch.c("Updated by"));
    let sql = format!(
        "SELECT substr(o.{c}, 12, 2), COALESCE((SELECT e.operator FROM ship_events e WHERE e.rn = o.rn AND e.kind = ? AND DATE_FORMAT(e.at, '%Y-%m-%d %H:%i') = o.{c} \
         ORDER BY e.id DESC LIMIT 1), o.{upd}), COUNT(*) FROM orders_text o WHERE substr(o.{c}, 1, 10) = ? GROUP BY 1, 2"
    );
    let rows = sqlx::query(&sql).bind(kind).bind(day).fetch_all(&s.db).await?;
    Ok(rows.iter().map(|r| (r.try_get_unchecked::<String, _>(0).unwrap_or_default(), r.try_get_unchecked::<String, _>(1).unwrap_or_default(), r.try_get_unchecked::<i64, _>(2).unwrap_or(0))).collect())
}

async fn count(s: &AppState, sql: &str, args: &[&str]) -> AppResult<i64> {
    let mut q = sqlx::query(sql);
    for a in args {
        q = q.bind(*a);
    }
    Ok(q.fetch_one(&s.db).await?.try_get_unchecked::<i64, _>(0).unwrap_or(0))
}

pub async fn dashboard(_u: CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let day = q.get("date").filter(|d| !d.is_empty()).cloned().unwrap_or_else(|| clock::now().format("%Y-%m-%d").to_string());
    if day.len() != 10 || NaiveDate::parse_from_str(&day, "%Y-%m-%d").is_err() {
        return Err(AppError::bad("Pick a date."));
    }
    let ops = operators(&s).await?;
    let on = |h: &str| format!("substr({}, 1, 10) = ?", sch.c(h));
    let c = |h: &str| sch.c(h);

    let prev = NaiveDate::parse_from_str(&day, "%Y-%m-%d").map(|d| d - chrono::Duration::days(1)).unwrap_or_default();
    let (usps_from, usps_to) = (format!("{} 14:00", prev.format("%Y-%m-%d")), format!("{day} 14:00"));
    let hourly = hours(&by_hour(&s, "SHIPPED TO  CUSTOMER DATE", "customer", &day).await?, &ops);
    let store = hours(&by_hour(&s, "SHIPPED TO  AMAZON DATE", "amazon", &day).await?, &ops);
    let cards = json!([
        { "key": "estimated", "label": "Estimated Delivery", "n": count(&s, &format!("SELECT COUNT(*) FROM orders_text WHERE {}", on("Delivery Date")), &[&day]).await? },
        { "key": "warehouse", "label": "Total Packages in Warehouse", "note": "right now, not by day", "n": count(&s, &format!(
            "SELECT COUNT(*) FROM orders_text WHERE {} = 'Delivered' AND {} != 'TRUE' AND {} != 'TRUE' AND {} = 'Preshipment'",
            c("Source Order Status"), c("Shipped to Customer"), c("Package Not Received"), c("Tracking Status")), &[]).await? },
        { "key": "hold", "label": "On Hold Orders", "n": count(&s, &format!("SELECT COUNT(*) FROM orders_text WHERE {}", on("Hold Date")), &[&day]).await? },
        { "key": "customer", "label": "Total Shipped to Customer", "n": total_of(&hourly) },
        // the USPS truck comes at 2 PM: what was shipped to the customer since the last pick-up (2:00 PM the day before until 1:59 PM that day)
        { "key": "usps", "label": "Packages Picked Up by USPS", "note": "shipped from 2 PM the day before to 1:59 PM", "href": format!("#orders?uspsday={day}"), "n": count(&s, &format!(
            "SELECT COUNT(*) FROM orders_text WHERE {col} >= ? AND {col} < ?", col = c("SHIPPED TO  CUSTOMER DATE")), &[&usps_from, &usps_to]).await? },
        // orders that take long from the Source Order Date to the Delivery Date: 4 or more days (0 to 3 days are not counted), leaving out the
        // orders that are delivered, refunded, for refund, cancelled or unfulfilled; right now, not by day
        { "key": "sourcetf", "label": "Source Time Frame", "note": "4 or more Source Delivery Days, right now. Not counted: delivered, refunded, for refund, cancelled, unfulfilled", "href": "#orders?show=sourcedays",
          "n": count(&s, "SELECT COUNT(*) FROM orders WHERE source_delivery_days >= 4 AND LOWER(TRIM(IFNULL(source_order_status, ''))) NOT IN ('for refund', 'refunded', 'cancelled', 'unfulfilled', 'delivered')", &[]).await? },
        { "key": "amazon", "label": "Total Shipped to Amazon", "n": count(&s, &format!("SELECT COUNT(*) FROM orders_text WHERE {}", on("SHIPPED TO  AMAZON DATE")), &[&day]).await? },
        // counted by the Delivery Date (the estimated delivery date from the Amazon file), for the rows that are Delivered
        { "key": "arrived", "label": "Packages Arrived Today", "n": count(&s, &format!("SELECT COUNT(*) FROM orders_text WHERE {} = 'Delivered' AND {}", c("Source Order Status"), on("Delivery Date")), &[&day]).await? },
    ]);

    // the sheet's counter columns (RTS / RTA / Item on Hold / Store Returns / All Orders / Stock Counter): live totals, not by day
    let open = format!("{} != 'TRUE' AND {} != 'TRUE'", c("Shipped to Amazon"), c("Stock"));
    let status = |v: &str| format!("SELECT COUNT(*) FROM orders_text WHERE {} = '{v}' AND {open}", c("Tracking Status"));
    let has_order = format!("{} != ''", c("Store Order No."));
    let counters = json!([
        { "key": "rts", "label": "RTS Counter", "note": "Return to Sender, not yet shipped to Amazon", "n": count(&s, &status("Return to Sender"), &[]).await? },
        { "key": "rta", "label": "RTA Counter", "note": "Return to Amazon, not yet shipped to Amazon", "n": count(&s, &status("Return to Amazon"), &[]).await? },
        { "key": "repo", "label": "RE-P.O Counter", "note": "RE-P.O ticked and Tracking Status still Preshipment, right now", "href": "#orders?show=repo", "n": count(&s, &format!("SELECT COUNT(*) FROM orders_text WHERE {} = 'TRUE' AND {} = 'Preshipment'", c("RE-P.O"), c("Tracking Status")), &[]).await? },
        { "key": "holdc", "label": "Item on Hold Counter", "note": "Items ticked on hold", "n": count(&s, &format!("SELECT COUNT(*) FROM orders_text WHERE {} = 'TRUE'", c("ITEM ON HOLD")), &[]).await? },
        { "key": "storec", "label": "Store Returns Counter", "note": "Store Returns, not yet shipped to Amazon", "n": count(&s, &status("Store Returns"), &[]).await? },
        { "key": "allc", "label": "All Orders Counter", "note": "Orders not yet shipped to the customer", "n": count(&s, &format!("SELECT COUNT(*) FROM orders_text WHERE {has_order} AND {} != 'TRUE'", c("Shipped to Customer")), &[]).await? },
        { "key": "stockc", "label": "Stock Counter", "note": "Orders taken from stock", "n": count(&s, &format!("SELECT COUNT(*) FROM orders_text WHERE {has_order} AND {} = 'TRUE'", c("Stock")), &[]).await? },
    ]);

    // how many rows have each row colour right now (the same rules All Orders paints and filters by)
    let fill = fill_expr(&sch, sch.at("Label Number").unwrap_or(0));
    let rows = sqlx::query(&format!("SELECT {fill} AS f, COUNT(*) FROM orders_text GROUP BY f")).fetch_all(&s.db).await?;
    let colors: serde_json::Map<String, Value> = rows
        .iter()
        .filter_map(|r| {
            let f = r.try_get_unchecked::<String, _>(0).unwrap_or_default();
            (!f.is_empty()).then(|| (f, json!(r.try_get_unchecked::<i64, _>(1).unwrap_or(0))))
        })
        .collect();

    // stock sorts: an operator put a Stock No. on an item
    let sorts = sqlx::query("SELECT DATE_FORMAT(at, '%H'), operator, COUNT(*) FROM stock_sorts WHERE DATE(at) = ? GROUP BY 1, 2").bind(&day).fetch_all(&s.db).await?;
    let sorts: Vec<(String, String, i64)> = sorts.iter().map(|r| (r.try_get_unchecked::<String, _>(0).unwrap_or_default(), r.try_get_unchecked::<String, _>(1).unwrap_or_default(), r.try_get_unchecked::<i64, _>(2).unwrap_or(0))).collect();
    let sorting = hours(&sorts, &ops);

    let now = clock::now();
    Ok(Json(json!({
        "date": day, "cards": cards, "counters": counters, "operators": ops.iter().map(|o| o.label.clone()).collect::<Vec<_>>(),
        "orders": hourly, "returns": store, "orders_total": total_of(&hourly), "returns_total": total_of(&store),
        "colors": colors, "operators_list": operators_json(&ops), "sorting_total": total_of(&sorting), "sorting": sorting,
        "today": now.format("%Y-%m-%d").to_string(), "now": now.format("%Y-%m-%d %H:%M").to_string(),
    })))
}
