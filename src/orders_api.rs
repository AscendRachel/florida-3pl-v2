//! All Orders, reading: the columns and summary cards (`/api/meta`), the rows (`/api/orders`), the values and colours of one
//! column's filter menu, the duplicate highlighting, the history of a cell, filter views. Writing a cell comes with the edit API.

use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use axum::response::Json;
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::auth::CurrentUser;
use crate::cformat;
use crate::error::{AppError, AppResult};
use crate::filters::{self, bind_args, CARDS};
use crate::{settings, AppState};

/// A text cell of the view, whatever type MariaDB reports for it.
fn cell(row: &sqlx::mysql::MySqlRow, i: usize) -> String {
    row.try_get_unchecked::<String, _>(i).unwrap_or_default()
}

fn num(p: &Value, key: &str, default: i64) -> i64 {
    match p.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Value::String(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

/// A GET query string as the same value a POST body gives (`flt` arrives as JSON text).
pub fn query_value(q: HashMap<String, String>) -> Value {
    let mut m = Map::new();
    for (k, v) in q {
        if k == "flt" {
            m.insert(k, serde_json::from_str(&v).unwrap_or_else(|_| json!({})));
        } else {
            m.insert(k, Value::String(v));
        }
    }
    Value::Object(m)
}

// ---------------------------------------------------------------- meta

pub async fn meta(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let sums: Vec<String> = CARDS
        .iter()
        .map(|(_, _, _, _, col, val)| match col {
            Some(h) => format!("CAST(COALESCE(SUM({} = '{}'), 0) AS SIGNED)", sch.c(h), val),
            None => "COUNT(*)".to_string(),
        })
        .collect();
    let row = sqlx::query(&format!("SELECT {} FROM orders_text", sums.join(", "))).fetch_one(&s.db).await?;
    let counts: Vec<i64> = (0..CARDS.len()).map(|i| row.try_get_unchecked::<i64, _>(i).or_else(|_| row.try_get_unchecked::<String, _>(i).map(|t| t.parse().unwrap_or(0))).unwrap_or(0)).collect();
    let cards: Vec<Value> = CARDS.iter().zip(&counts).map(|((k, l, h, t, _, _), n)| json!({ "key": k, "label": l, "hint": h, "tone": t, "n": n })).collect();
    let formula_cols = sch.formula_cols();
    let columns: Vec<Value> = sch
        .cols
        .iter()
        .filter(|c| !c.deleted)
        .map(|c| {
            let f = crate::schema::FORMULAS.iter().find(|(t, _)| *t == c.header).filter(|_| formula_cols.contains(&c.i));
            json!({
                "i": c.i, "name": c.label, "key": c.header, "kind": c.kind, "custom": c.custom, "protected": sch.protected(&c.header),
                "formula": f.map(|(_, src)| src.join(" + ")).unwrap_or_default(),
                "formula_from": f.map(|(_, src)| src.iter().filter_map(|x| sch.at(x)).collect::<Vec<_>>()).unwrap_or_default(),
            })
        })
        .collect();
    let imported = settings::get(&s.db, "orders.imported").await?;
    let locks = locks_public(&settings::get(&s.db, "locks").await?);
    Ok(Json(json!({ "columns": columns, "cards": cards, "total": counts[0], "imported": imported, "locks": locks })))
}

/// {defs, cols}: the lock definitions and which column each one protects.
fn locks_public(l: &Option<Value>) -> Value {
    let get = |path: &[&str]| path.iter().try_fold(l.as_ref()?, |v, k| v.get(*k)).cloned();
    json!({ "defs": get(&["defs"]).unwrap_or(json!({})), "cols": get(&["orders", "cols"]).unwrap_or(json!({})) })
}

// ---------------------------------------------------------------- rows

pub async fn orders_get(u: CurrentUser, st: State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    orders(u, st, Json(query_value(q))).await
}

pub async fn orders(_u: CurrentUser, State(s): State<AppState>, Json(p): Json<Value>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let w = filters::build_where(&sch, &p, None)?;
    let total: i64 = bind_args(sqlx::query(&format!("SELECT COUNT(*) FROM orders_text{}", w.sql)), &w.args).fetch_one(&s.db).await?.try_get(0)?;

    let sort = p.get("sort").and_then(Value::as_str).unwrap_or("");
    let desc = p.get("dir").and_then(Value::as_str) == Some("desc");
    let mut order = "rn".to_string();
    if sort == "rn" && desc {
        order = "rn DESC".into();
    } else if let Some(rest) = sort.strip_prefix("color:") {
        // one colour first, the rest after
        if let Some((j, c)) = rest.split_once(':') {
            let ok_color = c == "dup" || c == "none" || (c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit() && !x.is_ascii_uppercase()));
            if let (Ok(j), true) = (j.parse::<usize>(), ok_color) {
                if sch.active(j) {
                    let want = if c == "none" { "" } else { c };
                    order = format!("({} = '{}') DESC, rn", filters::fill_expr(&sch, j), want);
                }
            }
        }
    } else if let Ok(j) = sort.parse::<usize>() {
        if j < sch.n() {
            let expr = if sch.cols[j].kind == "num" { format!("CAST(c{j} AS DOUBLE)") } else { format!("c{j}") };
            order = format!("(c{j} = '') ASC, {expr} {}, rn", if desc { "DESC" } else { "ASC" });
        }
    }
    let offset = num(&p, "offset", 0).max(0);

    if p.get("rns_only").is_some_and(|v| v.as_bool().unwrap_or(false) || v.as_i64().unwrap_or(0) != 0) {
        let limit = num(&p, "limit", 1).max(1);
        let sql = format!("SELECT rn FROM orders_text{} ORDER BY {order} LIMIT {limit} OFFSET {offset}", w.sql);
        let rows = bind_args(sqlx::query(&sql), &w.args).fetch_all(&s.db).await?;
        return Ok(Json(json!({ "rns": rows.iter().map(|r| r.try_get::<u64, _>(0).unwrap_or(0)).collect::<Vec<_>>() })));
    }

    let limit = num(&p, "limit", 100).clamp(1, 500);
    let cols: Vec<String> = (0..sch.n()).map(|i| format!("c{i}")).collect();
    let sql = format!("SELECT rn, {} FROM orders_text{} ORDER BY {order} LIMIT {limit} OFFSET {offset}", cols.join(", "), w.sql);
    let rows = bind_args(sqlx::query(&sql), &w.args).fetch_all(&s.db).await?;
    let mut rns: Vec<u64> = vec![];
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            let rn = r.try_get::<u64, _>(0).unwrap_or(0);
            rns.push(rn);
            let mut v: Vec<Value> = vec![json!(rn)];
            v.extend((0..sch.n()).map(|i| Value::String(cell(r, i + 1))));
            Value::Array(v)
        })
        .collect();

    // dropdown cells and their lists, for the rows on this page
    let (mut dv, mut lists) = (Map::new(), Map::new());
    if !rns.is_empty() {
        let marks = vec!["?"; rns.len()].join(",");
        let sql = format!("SELECT rn, CAST(field AS CHAR), list_id FROM dropdown_cells WHERE rn IN ({marks})");
        let mut q = sqlx::query_as::<_, (u64, String, u32)>(&sql);
        for rn in &rns {
            q = q.bind(rn);
        }
        let mut ids: HashSet<u32> = HashSet::new();
        for (rn, field, list) in q.fetch_all(&s.db).await? {
            if let Some(c) = sch.cols.iter().find(|c| c.field == field) {
                dv.insert(format!("{rn}:{}", c.i), json!(list));
                ids.insert(list);
            }
        }
        if !ids.is_empty() {
            let sql = format!("SELECT id, CAST(options AS CHAR), strict FROM dropdown_lists WHERE id IN ({})", vec!["?"; ids.len()].join(","));
            let mut q = sqlx::query_as::<_, (u32, String, i8)>(&sql);
            for id in &ids {
                q = q.bind(id);
            }
            for (id, options, strict) in q.fetch_all(&s.db).await? {
                lists.insert(id.to_string(), json!({ "options": serde_json::from_str::<Value>(&options).unwrap_or(json!([])), "strict": strict != 0 }));
            }
        }
    }

    // locks: which of these rows are protected, and the definitions in use
    let stored = settings::get(&s.db, "locks").await?;
    let (mut row_locks, mut used) = (Map::new(), HashSet::<String>::new());
    if let Some(rows) = stored.as_ref().and_then(|l| l.pointer("/orders/rows")).and_then(Value::as_object) {
        for (lid, ranges) in rows {
            for r in ranges.as_array().into_iter().flatten() {
                let (a, b) = (r[0].as_u64().unwrap_or(1), r[1].as_u64().unwrap_or(0));
                for rn in rns.iter().filter(|rn| **rn >= a && **rn <= b) {
                    row_locks.insert(rn.to_string(), json!(lid));
                    used.insert(lid.clone());
                }
            }
        }
    }
    let mut lk = locks_public(&stored);
    if let Some(cols) = lk["cols"].as_object() {
        used.extend(cols.values().filter_map(|v| v.as_str().map(str::to_string)));
    }
    let defs: Map<String, Value> = lk["defs"].as_object().map(|d| d.iter().filter(|(k, _)| used.contains(*k)).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    lk["defs"] = Value::Object(defs);
    lk["rows"] = Value::Object(row_locks);

    Ok(Json(json!({ "total": total, "offset": offset, "rows": out, "dv": dv, "lists": lists, "locks": lk })))
}

// ---------------------------------------------------------------- the filter menu of a column

fn column_of(sch: &crate::schema::Schema, p: &Value) -> AppResult<usize> {
    let j = num(p, "col", 0);
    if j < 0 || !sch.active(j as usize) {
        return Err(AppError::bad("bad column"));
    }
    Ok(j as usize)
}

/// The fill colours that appear in one column, with how many rows have each (among the rows the OTHER filters leave visible).
pub async fn colors(_u: CurrentUser, State(s): State<AppState>, Json(p): Json<Value>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let j = column_of(&sch, &p)?;
    let w = filters::build_where(&sch, &p, Some(j))?;
    let expr = filters::fill_expr(&sch, j);
    if expr == "''" {
        return Ok(Json(json!({ "colors": [] })));
    }
    let sql = format!("SELECT {expr} AS k, COUNT(*) FROM orders_text{} GROUP BY k ORDER BY (k = ''), k", w.sql);
    let rows = bind_args(sqlx::query(&sql), &w.args).fetch_all(&s.db).await?;
    let colors: Vec<Value> = rows
        .iter()
        .map(|r| {
            let c = cell(r, 0);
            let n: i64 = r.try_get_unchecked(1).unwrap_or(0);
            let name = if c.is_empty() { "No colour" } else { cformat::color_name(&c) };
            json!({ "c": c, "n": n, "name": name })
        })
        .collect();
    Ok(Json(json!({ "colors": colors })))
}

/// The distinct values of one column with how many orders have each, among the orders that the OTHER filters leave visible.
pub async fn values(_u: CurrentUser, State(s): State<AppState>, Json(p): Json<Value>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let j = column_of(&sch, &p)?;
    let mut w = filters::build_where(&sch, &p, Some(j))?;
    let search = p.get("s").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
    if !search.is_empty() {
        let like: String = format!("%{}%", search.chars().flat_map(|c| if matches!(c, '%' | '_' | '|') { vec!['|', c] } else { vec![c] }).collect::<String>());
        w.sql.push_str(if w.sql.is_empty() { " WHERE " } else { " AND " });
        if sch.cols[j].kind == "date" {
            // stored as 2026-11-04 but shown as Nov 4, 2026: find a date by either
            w.sql.push_str(&format!("(LOWER(c{j}) LIKE ? ESCAPE '|' OR LOWER(DATE_FORMAT(STR_TO_DATE(substr(c{j}, 1, 10), '%Y-%m-%d'), '%b %e, %Y')) LIKE ? ESCAPE '|')"));
            w.args.push(filters::Arg::S(like.clone()));
        } else {
            w.sql.push_str(&format!("LOWER(c{j}) LIKE ? ESCAPE '|'"));
        }
        w.args.push(filters::Arg::S(like));
    }
    // sorted like the old app did: numbers by value, text by its letters without regard to capitals (binary order, so symbols come first)
    let order = if sch.cols[j].kind == "num" { "CAST(v AS DOUBLE)".to_string() } else { "LOWER(v) COLLATE utf8mb4_bin".to_string() };
    // distinct by exact text (capitals count), as the old app listed them
    let sql = format!("SELECT c{j} COLLATE utf8mb4_bin AS v, COUNT(*) FROM orders_text{} GROUP BY v ORDER BY (v <> ''), {order}, v LIMIT 501", w.sql);
    let rows = bind_args(sqlx::query(&sql), &w.args).fetch_all(&s.db).await?;
    let items: Vec<Value> = rows.iter().take(500).map(|r| json!({ "v": cell(r, 0), "n": r.try_get_unchecked::<i64, _>(1).unwrap_or(0) })).collect();
    Ok(Json(json!({ "values": items, "more": rows.len() > 500 })))
}

pub async fn colors_get(u: CurrentUser, st: State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    colors(u, st, Json(query_value(q))).await
}

pub async fn values_get(u: CurrentUser, st: State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    values(u, st, Json(query_value(q))).await
}

// ---------------------------------------------------------------- small lookups

/// For the duplicate highlighting of All Orders (the sheet's =countif(C:C,C2)>1): the values that occur more than once.
pub async fn duplicates(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let mut out = Map::new();
    for name in crate::cformat::DUP_COLUMNS {
        let c = sch.c(name);
        let rows: Vec<String> = sqlx::query_scalar(&format!("SELECT lower(trim({c})) v FROM orders_text WHERE trim({c}) != '' GROUP BY v HAVING COUNT(*) > 1")).fetch_all(&s.db).await?;
        out.insert(name.to_string(), json!(rows));
    }
    Ok(Json(json!({ "values": out })))
}

pub async fn history(_u: CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let rn: u64 = q.get("rn").and_then(|v| v.parse().ok()).unwrap_or(0);
    let col: i64 = q.get("col").and_then(|v| v.parse().ok()).unwrap_or(-1);
    if col < 0 || !sch.active(col as usize) {
        return Err(AppError::bad("That column does not exist."));
    }
    let rows = sqlx::query_as::<_, (chrono::NaiveDateTime, String, Option<String>, Option<String>, String)>(
        "SELECT at, who, CAST(old_value AS CHAR), CAST(new_value AS CHAR), note FROM edit_log WHERE rn = ? AND field = ? ORDER BY id DESC LIMIT 200",
    )
    .bind(rn)
    .bind(&sch.cols[col as usize].field)
    .fetch_all(&s.db)
    .await?;
    let events: Vec<Value> = rows.into_iter().map(|(at, who, old, new, note)| json!({ "at": at.format("%Y-%m-%d %H:%M").to_string(), "who": who, "old": old.unwrap_or_default(), "new": new.unwrap_or_default(), "note": note })).collect();
    Ok(Json(json!({ "events": events })))
}

pub async fn ship_events(_u: CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    let rn: u64 = q.get("rn").and_then(|v| v.parse().ok()).unwrap_or(0);
    let rows = sqlx::query_as::<_, (chrono::NaiveDateTime, String, String, String)>("SELECT at, CAST(kind AS CHAR), operator, note FROM ship_events WHERE rn = ? ORDER BY id DESC LIMIT 100")
        .bind(rn)
        .fetch_all(&s.db)
        .await?;
    let events: Vec<Value> = rows.into_iter().map(|(at, kind, operator, note)| json!({ "at": at.format("%Y-%m-%d %H:%M").to_string(), "kind": kind, "operator": operator, "note": note })).collect();
    Ok(Json(json!({ "events": events })))
}

// ---------------------------------------------------------------- saved filter views

pub async fn views_get(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "views": settings::get(&s.db, "views").await?.unwrap_or(json!([])) })))
}
