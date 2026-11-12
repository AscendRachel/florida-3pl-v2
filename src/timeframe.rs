//! Source Time Frame: the orders whose estimated delivery date (ETA) moved to another date at least once (a new Delivery Date from an
//! Amazon order import or an edit). One line per such order: its Source Order Date and Number, the first ETA that was recorded, the
//! current ETA, how many times the ETA moved, and how many days the ETA moved in all (New ETA minus First ETA).
//! Nothing is stored for this page: everything comes from the orders and from the edit history.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::Json;
use serde_json::{json, Value};
use sqlx::Row;

use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::{clock, AppState};

const PAGE: i64 = 100;

pub async fn list(_u: CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let (sod, son, eta) = (sch.c("Source Order Date"), sch.c("Source Order Number"), sch.c("Delivery Date"));

    // per order: how often the ETA moved to another date (the same rule as the Check List card) and the first ETA ever recorded
    // (the whole history counts, also after an order was marked resolved: resolving only hides the order until its ETA moves again)
    let history = "LEFT JOIN (SELECT e.rn, \
         SUM(CASE WHEN trim(IFNULL(e.old_value, '')) <> '' AND substr(trim(IFNULL(e.old_value, '')), 1, 10) <> substr(trim(IFNULL(e.new_value, '')), 1, 10) AND e.note <> 'Row added' THEN 1 ELSE 0 END) AS changes, \
         MAX(CASE WHEN trim(IFNULL(e.old_value, '')) <> '' AND substr(trim(IFNULL(e.old_value, '')), 1, 10) <> substr(trim(IFNULL(e.new_value, '')), 1, 10) AND e.note <> 'Row added' THEN e.id END) AS last_change, \
         SUBSTRING_INDEX(GROUP_CONCAT(CASE WHEN trim(IFNULL(e.old_value, '')) <> '' THEN trim(e.old_value) ELSE trim(IFNULL(e.new_value, '')) END ORDER BY e.id SEPARATOR '|'), '|', 1) AS first_eta \
         FROM edit_log e WHERE e.field = 'delivery_date' GROUP BY e.rn) a ON a.rn = o.rn LEFT JOIN timeframe_resolved r ON r.rn = o.rn";

    // only the orders that meet the rule: the ETA was moved to another date at least once
    let mut where_ = vec![format!("o.{son} <> ''"), "IFNULL(a.changes, 0) > 0".to_string(), "a.last_change > IFNULL(r.up_to_id, 0)".to_string()];
    let mut args: Vec<String> = vec![];
    let term = q.get("q").map(|t| t.trim()).unwrap_or("");
    if !term.is_empty() {
        where_.push(format!("(o.{son} LIKE ? OR o.{sod} LIKE ?)"));
        let like = format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        args.push(like.clone());
        args.push(like);
    }
    let where_sql = where_.join(" AND ");

    let dir = if q.get("dir").is_some_and(|d| d == "asc") { "ASC" } else { "DESC" };
    let order = match q.get("sort").map(String::as_str).unwrap_or("") {
        "son" => format!("o.{son} {dir}"),
        "first" => format!("first_eta {dir}"),
        "eta" => format!("o.{eta} {dir}"),
        "changes" => format!("changes {dir}"),
        "frame" => format!("frame {dir}"),
        _ => format!("o.{sod} {dir}"),
    };
    let page: i64 = q.get("page").and_then(|p| p.parse().ok()).filter(|p: &i64| *p >= 1).unwrap_or(1).min(100_000);

    let count_sql = format!("SELECT COUNT(*) FROM orders_text o {history} WHERE {where_sql}");
    let mut cq = sqlx::query(&count_sql);
    for a in &args {
        cq = cq.bind(a);
    }
    let total: i64 = cq.fetch_one(&s.db).await?.try_get_unchecked::<i64, _>(0).unwrap_or(0);

    let sql = format!(
        "SELECT o.rn, CAST(o.{sod} AS CHAR), CAST(o.{son} AS CHAR), CAST(a.first_eta AS CHAR) AS first_eta, CAST(o.{eta} AS CHAR), \
         CAST(IFNULL(a.changes, 0) AS SIGNED) AS changes, CAST(DATEDIFF(substr(o.{eta}, 1, 10), substr(a.first_eta, 1, 10)) AS SIGNED) AS frame \
         FROM orders_text o {history} WHERE {where_sql} ORDER BY {order}, o.rn DESC LIMIT {PAGE} OFFSET {}",
        (page - 1) * PAGE
    );
    let mut rq = sqlx::query(&sql);
    for a in &args {
        rq = rq.bind(a);
    }
    let rows = rq.fetch_all(&s.db).await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let t = |i: usize| r.try_get_unchecked::<String, _>(i).unwrap_or_default();
            json!({
                "rn": r.try_get_unchecked::<u64, _>(0).unwrap_or(0), "source_date": t(1), "source_order": t(2), "first_eta": t(3), "new_eta": t(4),
                "changes": r.try_get_unchecked::<i64, _>(5).unwrap_or(0), "frame": r.try_get_unchecked::<Option<i64>, _>(6).unwrap_or(None),
            })
        })
        .collect();
    Ok(Json(json!({ "items": items, "total": total, "page": page, "pages": ((total + PAGE - 1) / PAGE).max(1) })))
}

/// Mark an order resolved: it leaves the table until its ETA moves again. Answers with the earlier mark (if any) so the page can undo it.
pub async fn resolve(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let rn = b.get("rn").and_then(Value::as_u64).ok_or_else(|| AppError::bad("Which order?"))?;
    let latest: Option<i64> = sqlx::query_scalar("SELECT CAST(MAX(id) AS SIGNED) FROM edit_log WHERE rn = ? AND field = 'delivery_date'").bind(rn).fetch_one(&s.db).await?;
    let Some(latest) = latest else { return Err(AppError::bad("This order has no ETA change to resolve.")) };
    let prev: Option<i64> = sqlx::query_scalar("SELECT up_to_id FROM timeframe_resolved WHERE rn = ?").bind(rn).fetch_optional(&s.db).await?;
    sqlx::query("INSERT INTO timeframe_resolved (rn, up_to_id, resolved_by, resolved_at) VALUES (?,?,?,?) ON DUPLICATE KEY UPDATE up_to_id = VALUES(up_to_id), resolved_by = VALUES(resolved_by), resolved_at = VALUES(resolved_at)")
        .bind(rn)
        .bind(latest)
        .bind(&u.name)
        .bind(clock::now())
        .execute(&s.db)
        .await?;
    Ok(Json(json!({ "ok": true, "prev": prev })))
}

/// Undo a resolve: put back the earlier mark, or none.
pub async fn reopen(_u: CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let rn = b.get("rn").and_then(Value::as_u64).ok_or_else(|| AppError::bad("Which order?"))?;
    match b.get("prev").and_then(Value::as_i64) {
        Some(prev) => sqlx::query("UPDATE timeframe_resolved SET up_to_id = ? WHERE rn = ?").bind(prev).bind(rn).execute(&s.db).await?,
        None => sqlx::query("DELETE FROM timeframe_resolved WHERE rn = ?").bind(rn).execute(&s.db).await?,
    };
    Ok(Json(json!({ "ok": true })))
}
