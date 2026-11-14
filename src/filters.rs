//! The search box, the summary cards and the column filters of All Orders, as SQL on the text view `orders_text`.
//! (A port of the Python app's `build_where` / `column_conditions`.)

use chrono::{Duration, NaiveDate};
use serde_json::Value;
use sqlx::mysql::MySqlArguments;
use sqlx::query::Query;
use sqlx::MySql;

use crate::error::{AppError, AppResult};
use crate::schema::Schema;
use crate::{cformat, clock};

pub enum Arg {
    S(String),
    F(f64),
}

pub struct Built {
    /// " WHERE (...) AND (...)", or "" when nothing filters.
    pub sql: String,
    pub args: Vec<Arg>,
}

/// Quick-filter cards on top of the page: (key, label, hint, tone, column header, value).
pub const CARDS: [(&str, &str, &str, &str, Option<&str>, &str); 12] = [
    ("all", "All orders", "Every order", "neutral", None, ""),
    ("delivered", "Delivered", "Tracking status", "ok", Some("Tracking Status"), "Delivered"),
    ("transit", "In transit", "Tracking status", "info", Some("Tracking Status"), "In Transit"),
    ("preship", "Pre-shipment", "Label made, not yet scanned", "violet", Some("Tracking Status"), "Preshipment"),
    ("shipped", "Shipped to customer", "Ticked on the sheet", "ok", Some("Shipped to Customer"), "TRUE"),
    ("amazon", "Shipped to Amazon", "Ticked on the sheet", "info", Some("Shipped to Amazon"), "TRUE"),
    ("hold", "Item on hold", "Needs a decision", "warn", Some("ITEM ON HOLD"), "TRUE"),
    ("void", "To void", "Label to cancel", "crit", Some("To Void"), "TRUE"),
    ("notrec", "Package not received", "Waiting at the warehouse", "crit", Some("Package Not Received"), "TRUE"),
    ("stock", "Stock", "Item taken from stock", "violet", Some("Stock"), "TRUE"),
    ("returns", "Store returns", "Tracking status", "warn", Some("Tracking Status"), "Store Returns"),
    ("rts", "Return to sender", "Tracking status", "warn", Some("Tracking Status"), "Return to Sender"),
];

/// Binds the arguments of a built filter to a query, in order.
pub fn bind_args<'q>(mut q: Query<'q, MySql, MySqlArguments>, args: &'q [Arg]) -> Query<'q, MySql, MySqlArguments> {
    for a in args {
        q = match a {
            Arg::S(s) => q.bind(s.as_str()),
            Arg::F(f) => q.bind(*f),
        };
    }
    q
}

/// A LIKE pattern for "contains" (the text lower-cased; % _ and | are escaped with |).
pub fn like_arg(text: &str) -> String {
    let mut out = String::from("%");
    for c in text.to_lowercase().chars() {
        if matches!(c, '%' | '_' | '|') {
            out.push('|');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn is_iso(s: &str) -> bool {
    let ok_date = s.get(..10).is_some_and(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok());
    ok_date && (s.len() == 10 || (s.len() == 16 && s.as_bytes()[10] == b' ' && chrono::NaiveTime::parse_from_str(&s[11..], "%H:%M").is_ok()))
}

fn st(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// "today" and "a day ago" in US Eastern, as the colour rules need them.
pub fn fill_times() -> (String, String) {
    let now = clock::now();
    (now.format("%Y-%m-%d").to_string(), (now - Duration::days(1)).format("%Y-%m-%d %H:%M").to_string())
}

pub fn fill_expr(schema: &Schema, j: usize) -> String {
    let (today, day_ago) = fill_times();
    cformat::fill_sql(schema, &schema.cols[j].header, &today, &day_ago)
}

/// SQL for one column's filter: f = {"c": {op, a, b} (condition), "v": {mode: "in"|"out", list: [...]} (values), "k": {list: [colours]}}.
fn column_conditions(schema: &Schema, j: usize, f: &Value) -> AppResult<(Vec<String>, Vec<Arg>)> {
    let (mut out, mut args) = (vec![], vec![]);
    let col = format!("c{j}");
    let kind = schema.cols[j].kind.as_str();
    let label = &schema.cols[j].label;
    if let Some(c) = f.get("c").filter(|c| c.get("op").and_then(Value::as_str).is_some_and(|o| !o.is_empty())) {
        let (op, a, b) = (st(c, "op"), st(c, "a"), st(c, "b"));
        let op = op.as_str();
        if op == "empty" {
            out.push(format!("{col} = ''"));
        } else if op == "notempty" {
            out.push(format!("{col} <> ''"));
        } else if kind == "num" {
            let num = |t: &str| t.replace(',', "").parse::<f64>().ok();
            let x = num(&a).ok_or_else(|| AppError::bad(format!("{label}: enter a number for the filter.")))?;
            if op == "between" {
                let y = num(&b).ok_or_else(|| AppError::bad(format!("{label}: enter a number for the filter.")))?;
                out.push(format!("{col} <> '' AND CAST({col} AS DOUBLE) BETWEEN ? AND ?"));
                args.extend([Arg::F(x.min(y)), Arg::F(x.max(y))]);
            } else if let Some(sym) = [("eq", "="), ("neq", "<>"), ("gt", ">"), ("gte", ">="), ("lt", "<"), ("lte", "<=")].iter().find(|(o, _)| *o == op).map(|(_, s)| *s) {
                out.push(format!("{col} <> '' AND CAST({col} AS DOUBLE) {sym} ?"));
                args.push(Arg::F(x));
            }
        } else if kind == "date" {
            if !is_iso(&a) {
                return Err(AppError::bad(format!("{label}: pick a date for the filter.")));
            }
            let expr = format!("substr({col}, 1, 10)");
            let guard = format!("{col} REGEXP '^[0-9]{{4}}-'");
            if op == "between" {
                if b.get(..10).is_none_or(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err()) {
                    return Err(AppError::bad(format!("{label}: pick the second date too.")));
                }
                out.push(format!("{guard} AND {expr} BETWEEN ? AND ?"));
                let (p, q) = (a[..10].to_string(), b[..10].to_string());
                args.extend([Arg::S(p.clone().min(q.clone())), Arg::S(p.max(q))]);
            } else if let Some(sym) = [("eq", "="), ("lt", "<"), ("gt", ">")].iter().find(|(o, _)| *o == op).map(|(_, s)| *s) {
                out.push(format!("{guard} AND {expr} {sym} ?"));
                args.push(Arg::S(a[..10].to_string()));
            }
        } else {
            match op {
                "contains" | "notcontains" | "starts" | "ends" => {
                    let like = like_arg(&a);
                    let like = match op {
                        "starts" => like[1..].to_string(),
                        "ends" => like[..like.len() - 1].to_string(),
                        _ => like,
                    };
                    let neg = if op == "notcontains" { "NOT " } else { "" };
                    out.push(format!("LOWER({col}) {neg}LIKE ? ESCAPE '|'"));
                    args.push(Arg::S(like));
                }
                "eq" => {
                    out.push(format!("LOWER({col}) = ?"));
                    args.push(Arg::S(a.to_lowercase()));
                }
                _ => {}
            }
        }
    }
    // filter by colour: only the cells that are painted one of these colours ("" is no colour)
    if let Some(list) = f.get("k").and_then(|k| k.get("list")).and_then(Value::as_array).filter(|l| !l.is_empty()) {
        let picks: Vec<String> = list.iter().filter_map(Value::as_str).take(40).map(|s| s.chars().take(12).collect()).collect();
        out.push(format!("{} IN ({})", fill_expr(schema, j), vec!["?"; picks.len()].join(",")));
        args.extend(picks.into_iter().map(Arg::S));
    }
    if let Some(v) = f.get("v").filter(|v| v.is_object()) {
        let vals: Vec<String> = v.get("list").and_then(Value::as_array).map(|l| l.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect()).unwrap_or_default();
        if vals.len() > 20000 {
            return Err(AppError::bad("Too many values are selected in one filter."));
        }
        // an exact match (the old app was case-sensitive here, so the binary collation keeps it so)
        let exact = format!("{col} COLLATE utf8mb4_bin");
        if v.get("mode").and_then(Value::as_str) == Some("in") {
            if vals.is_empty() {
                out.push("0".into());
            } else {
                out.push(format!("{exact} IN ({})", vec!["?"; vals.len()].join(",")));
                args.extend(vals.into_iter().map(Arg::S));
            }
        } else if !vals.is_empty() {
            out.push(format!("{exact} NOT IN ({})", vec!["?"; vals.len()].join(",")));
            args.extend(vals.into_iter().map(Arg::S));
        }
    }
    Ok((out, args))
}

/// p: {q, card, flt: {column number: filter}}. `skip` leaves one column's own filter out (for its value list).
pub fn build_where(schema: &Schema, p: &Value, skip: Option<usize>) -> AppResult<Built> {
    let (mut where_, mut args) = (vec![], vec![]);
    for term in st(p, "q").to_lowercase().split_whitespace() {
        where_.push("_s LIKE ?".to_string());
        args.push(Arg::S(format!("%{term}%")));
    }
    // the Team Daily Check List card "Estimated Delivery Changed": the orders whose Delivery Date was moved to another date on that day
    // (the same rule as the card's count in checklist.rs)
    if let Ok(day) = NaiveDate::parse_from_str(&st(p, "etaday"), "%Y-%m-%d") {
        let start = day.and_hms_opt(0, 0, 0).unwrap_or_default();
        where_.push(
            "rn IN (SELECT rn FROM edit_log WHERE at >= ? AND at < ? AND field = 'delivery_date' AND trim(IFNULL(old_value, '')) <> ''              AND substr(trim(IFNULL(old_value, '')), 1, 10) <> substr(trim(IFNULL(new_value, '')), 1, 10) AND note <> 'Row added')"
                .to_string(),
        );
        args.push(Arg::S(start.format("%Y-%m-%d %H:%M:%S").to_string()));
        args.push(Arg::S((start + Duration::days(1)).format("%Y-%m-%d %H:%M:%S").to_string()));
    }
    // the Dashboard card "Packages Picked Up by USPS": shipped to the customer from 2:00 PM the day before until 1:59 PM on that day
    if let Ok(day) = NaiveDate::parse_from_str(&st(p, "uspsday"), "%Y-%m-%d") {
        let col = schema.c("SHIPPED TO  CUSTOMER DATE");
        where_.push(format!("{col} >= ? AND {col} < ?"));
        args.push(Arg::S(format!("{} 14:00", (day - Duration::days(1)).format("%Y-%m-%d"))));
        args.push(Arg::S(format!("{} 14:00", day.format("%Y-%m-%d"))));
    }
    // the Dashboard card "RE-P.O Counter": the rows whose RE-P.O was ticked on that day
    if let Ok(day) = NaiveDate::parse_from_str(&st(p, "repoday"), "%Y-%m-%d") {
        where_.push(format!("substr({}, 1, 10) = ?", schema.c("RE-P.O Date")));
        args.push(Arg::S(day.format("%Y-%m-%d").to_string()));
    }
    let card = st(p, "card");
    for (key, _, _, _, col, val) in CARDS {
        if key == card {
            if let Some(h) = col {
                where_.push(format!("{} = ?", schema.c(h)));
                args.push(Arg::S(val.to_string()));
            }
        }
    }
    if let Some(flt) = p.get("flt").and_then(Value::as_object) {
        let mut keys: Vec<(&String, usize)> = flt.keys().filter_map(|k| k.parse::<usize>().ok().map(|i| (k, i))).collect();
        keys.sort_by_key(|(_, i)| *i);
        for (k, j) in keys {
            if j >= schema.n() || !schema.active(j) || Some(j) == skip || !flt[k].is_object() {
                continue;
            }
            let (w, a) = column_conditions(schema, j, &flt[k])?;
            where_.extend(w);
            args.extend(a);
        }
    }
    // `keep`: rows the person changed on this screen stay in the list even when the change made them stop matching the filter
    // (they are dropped when the person changes the filter or leaves the screen)
    let keep: Vec<u64> = p.get("keep").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).take(1000).collect()).unwrap_or_default();
    let sql = if where_.is_empty() {
        String::new()
    } else {
        let all = where_.iter().map(|w| format!("({w})")).collect::<Vec<_>>().join(" AND ");
        if keep.is_empty() {
            format!(" WHERE {all}")
        } else {
            args.extend(keep.iter().map(|rn| Arg::S(rn.to_string())));
            format!(" WHERE (({all}) OR rn IN ({}))", vec!["?"; keep.len()].join(","))
        }
    };
    Ok(Built { sql, args })
}
