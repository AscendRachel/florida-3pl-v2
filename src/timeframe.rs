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
use crate::error::AppResult;
use crate::AppState;

const PAGE: i64 = 100;

pub async fn list(_u: CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let (sod, son, eta) = (sch.c("Source Order Date"), sch.c("Source Order Number"), sch.c("Delivery Date"));

    // per order: how often the ETA moved to another date (the same rule as the Check List card) and the first ETA ever recorded
    let history = "LEFT JOIN (SELECT rn, \
         SUM(CASE WHEN trim(IFNULL(old_value, '')) <> '' AND substr(trim(IFNULL(old_value, '')), 1, 10) <> substr(trim(IFNULL(new_value, '')), 1, 10) AND note <> 'Row added' THEN 1 ELSE 0 END) AS changes, \
         SUBSTRING_INDEX(GROUP_CONCAT(CASE WHEN trim(IFNULL(old_value, '')) <> '' THEN trim(old_value) ELSE trim(IFNULL(new_value, '')) END ORDER BY id SEPARATOR '|'), '|', 1) AS first_eta \
         FROM edit_log WHERE field = 'delivery_date' GROUP BY rn) a ON a.rn = o.rn";

    // only the orders that meet the rule: the ETA was moved to another date at least once
    let mut where_ = vec![format!("o.{son} <> ''"), "IFNULL(a.changes, 0) > 0".to_string()];
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
