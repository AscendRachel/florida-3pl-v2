//! Use Stock: send a product to a customer order from the stock that was set aside (returned items kept with a STOCK NO.).
//!
//! A stock item is an order of All Orders with Stock ticked, or a row of Kohl's Returns. Its quantity is its QTY. Using stock never changes
//! that QTY: each use is one row of `stock_used`, and what is left is QTY minus the rows that are not voided. At 0 the item leaves the
//! Stocks cards; the rows stay in the "Used" list (an undo marks a row voided, it is never deleted).

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::Json;
use serde_json::{json, Value};
use sqlx::{MySqlConnection, Row};

use crate::auth::{need_tab, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::orders_write::now_minute;
use crate::AppState;

const TAB: &str = "stockuse";

fn clip(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

fn text(r: &sqlx::mysql::MySqlRow, i: usize) -> String {
    r.try_get_unchecked::<String, _>(i).unwrap_or_default()
}

/// A stock item's quantity: its QTY, and 1 when that is blank or not a number.
fn qty_of(v: &str) -> i64 {
    v.trim().replace(',', "").parse::<f64>().map(|f| f.round() as i64).ok().filter(|n| *n > 0).unwrap_or(1)
}

async fn used_of(db: &mut MySqlConnection, source: &str, id: i64) -> AppResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT CAST(COALESCE(SUM(qty), 0) AS SIGNED) FROM stock_used WHERE source = ? AND ref = ? AND voided = 0")
        .bind(source)
        .bind(id)
        .fetch_one(db)
        .await?)
}

/// (source, id, stock no, sku, product, store, image, qty text) of one stock item -> what the pages show.
#[allow(clippy::too_many_arguments)]
async fn shape(db: &mut MySqlConnection, source: &str, id: i64, stock_no: String, sku: String, product: String, store: String, image: String, qty: &str) -> AppResult<Value> {
    let (total, used) = (qty_of(qty), used_of(db, source, id).await?);
    Ok(json!({ "source": source, "ref": id, "stock_no": stock_no, "sku": sku, "product": product, "store": store, "image": image,
               "qty": total, "used": used, "remaining": (total - used).max(0) }))
}

/// One stock item, or None when it is gone or no longer stock.
async fn item(db: &mut MySqlConnection, s: &AppState, source: &str, id: i64) -> AppResult<Option<Value>> {
    if source == "kohls" {
        let row = sqlx::query("SELECT CAST(id AS SIGNED), CAST(stock_no AS CHAR), CAST(sku AS CHAR), CAST(IFNULL(product,'') AS CHAR), CAST(store AS CHAR), CAST(IFNULL(image,'') AS CHAR), CAST(IFNULL(qty,'') AS CHAR) FROM kohls WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
        let Some(r) = row else { return Ok(None) };
        return Ok(Some(shape(db, "kohls", id, text(&r, 1), text(&r, 2), text(&r, 3), text(&r, 4), text(&r, 5), &text(&r, 6)).await?));
    }
    let sch = s.schema();
    let sql = format!(
        "SELECT rn, {}, {}, {}, {}, {}, {}, {} FROM orders_text WHERE rn = ?",
        sch.c("STOCK NO."), sch.c("SKU"), sch.c("Product Name"), sch.c("Store"), sch.c("Product Image Link"), sch.c("QTY"), sch.c("Stock")
    );
    let Some(r) = sqlx::query(&sql).bind(id).fetch_optional(&mut *db).await? else { return Ok(None) };
    if text(&r, 7) != "TRUE" || text(&r, 1).trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(shape(db, "order", id, text(&r, 1), text(&r, 2), text(&r, 3), text(&r, 4), text(&r, 5), &text(&r, 6)).await?))
}

/// The stock items with this STOCK NO. (and how much of each is left).
pub async fn find(CurrentUser(u): CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    need_tab(&u, TAB)?;
    let no = clip(q.get("stock_no").map(String::as_str).unwrap_or(""), 100);
    if no.is_empty() {
        return Ok(Json(json!({ "items": [] })));
    }
    let sch = s.schema();
    let mut db = s.db.acquire().await?;
    let sql = format!("SELECT rn FROM orders_text WHERE LOWER(TRIM({})) = LOWER(?) AND {} = 'TRUE' ORDER BY rn DESC LIMIT 50", sch.c("STOCK NO."), sch.c("Stock"));
    let rns: Vec<i64> = sqlx::query(&sql).bind(&no).fetch_all(&mut *db).await?.iter().map(|r| r.try_get_unchecked::<i64, _>(0).unwrap_or(0)).collect();
    let ids: Vec<i64> = sqlx::query("SELECT CAST(id AS SIGNED) FROM kohls WHERE LOWER(TRIM(stock_no)) = LOWER(?) ORDER BY id DESC LIMIT 50")
        .bind(&no)
        .fetch_all(&mut *db)
        .await?
        .iter()
        .map(|r| r.try_get_unchecked::<i64, _>(0).unwrap_or(0))
        .collect();
    let mut items = vec![];
    for (source, list) in [("order", rns), ("kohls", ids)] {
        for id in list {
            if let Some(v) = item(&mut db, &s, source, id).await? {
                items.push(v);
            }
        }
    }
    Ok(Json(json!({ "items": items })))
}

/// The customer order a person typed (a Store Order No. or a Label Number), so they can see it is the right one.
pub async fn order(CurrentUser(u): CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    need_tab(&u, TAB)?;
    let no = clip(q.get("ref").map(String::as_str).unwrap_or(""), 200);
    if no.is_empty() {
        return Ok(Json(json!({ "orders": [] })));
    }
    let sch = s.schema();
    let (so, lb) = (sch.c("Store Order No."), sch.c("Label Number"));
    let sql = format!(
        "SELECT {lb}, {so}, {}, {}, {}, {} FROM orders_text WHERE LOWER(TRIM({so})) = LOWER(?) OR LOWER(TRIM({lb})) = LOWER(?) ORDER BY rn DESC LIMIT 5",
        sch.c("Customer Name"), sch.c("Product Name"), sch.c("SKU"), sch.c("Store")
    );
    let rows = sqlx::query(&sql).bind(&no).bind(&no).fetch_all(&s.db).await?;
    let orders: Vec<Value> = rows.iter().map(|r| json!({ "label": text(r, 0), "store_order": text(r, 1), "customer": text(r, 2), "product": text(r, 3), "sku": text(r, 4), "store": text(r, 5) })).collect();
    Ok(Json(json!({ "orders": orders })))
}

/// Take `qty` of a stock item for a customer order.
pub async fn use_stock(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    need_tab(&u, TAB)?;
    let source = b.get("source").and_then(Value::as_str).unwrap_or("");
    if source != "order" && source != "kohls" {
        return Err(AppError::bad("Pick the stock item first."));
    }
    let id = b.get("ref").and_then(Value::as_i64).unwrap_or(0);
    let qty = b.get("qty").and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|t| t.trim().parse().ok()))).unwrap_or(0);
    let order_ref = clip(b.get("order_ref").and_then(Value::as_str).unwrap_or(""), 200);
    let note = clip(b.get("note").and_then(Value::as_str).unwrap_or(""), 500);
    if qty < 1 {
        return Err(AppError::bad("Enter how many to use (1 or more)."));
    }
    if order_ref.is_empty() {
        return Err(AppError::bad("Enter the customer order this stock goes to."));
    }

    let mut tx = s.db.begin().await?;
    // two people using the same stock item at once wait for each other here, so the quantity can never go below 0
    let lock = if source == "kohls" { "SELECT 1 FROM kohls WHERE id = ? FOR UPDATE" } else { "SELECT 1 FROM orders WHERE rn = ? FOR UPDATE" };
    sqlx::query(lock).bind(id).fetch_optional(&mut *tx).await?;
    let Some(it) = item(&mut tx, &s, source, id).await? else { return Err(AppError::bad("That item is no longer in stock.")) };
    let left = it["remaining"].as_i64().unwrap_or(0);
    if qty > left {
        return Err(AppError::bad(if left == 0 { "There is none of this stock left.".to_string() } else { format!("Only {left} left in stock.") }));
    }
    sqlx::query("INSERT INTO stock_used (at, operator, source, ref, stock_no, sku, product, qty, order_ref, note) VALUES (?,?,?,?,?,?,?,?,?,?)")
        .bind(now_minute())
        .bind(&u.name)
        .bind(source)
        .bind(id)
        .bind(it["stock_no"].as_str().unwrap_or(""))
        .bind(it["sku"].as_str().unwrap_or(""))
        .bind(it["product"].as_str().unwrap_or(""))
        .bind(qty)
        .bind(&order_ref)
        .bind(&note)
        .execute(&mut *tx)
        .await?;
    let after = item(&mut tx, &s, source, id).await?;
    tx.commit().await?;
    s.bump();
    Ok(Json(json!({ "item": after })))
}

/// What has been used, newest first.
pub async fn used(CurrentUser(u): CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    need_tab(&u, TAB)?;
    let word = clip(q.get("q").map(String::as_str).unwrap_or(""), 100).to_lowercase();
    let like = format!("%{}%", word.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query(
        "SELECT CAST(id AS SIGNED), CAST(DATE_FORMAT(at, '%Y-%m-%d %H:%i') AS CHAR), CAST(operator AS CHAR), CAST(source AS CHAR), CAST(stock_no AS CHAR), CAST(sku AS CHAR), CAST(IFNULL(product,'') AS CHAR), \
         CAST(qty AS SIGNED), CAST(order_ref AS CHAR), CAST(IFNULL(note,'') AS CHAR) FROM stock_used WHERE voided = 0 \
         AND (? = '' OR LOWER(CONCAT_WS(' ', stock_no, sku, product, order_ref, operator, note)) LIKE ?) ORDER BY id DESC LIMIT 500",
    )
    .bind(&word)
    .bind(&like)
    .fetch_all(&s.db)
    .await?;
    let total: i64 = rows.iter().map(|r| r.try_get_unchecked::<i64, _>(7).unwrap_or(0)).sum();
    let rows: Vec<Value> = rows
        .iter()
        .map(|r| {
            let by = text(r, 2);
            json!({ "id": r.try_get_unchecked::<i64, _>(0).unwrap_or(0), "at": text(r, 1), "by": by, "source": text(r, 3), "stock_no": text(r, 4), "sku": text(r, 5), "product": text(r, 6),
                    "qty": r.try_get_unchecked::<i64, _>(7).unwrap_or(0), "order_ref": text(r, 8), "note": text(r, 9), "can_undo": u.is_admin() || by == u.name })
        })
        .collect();
    Ok(Json(json!({ "rows": rows, "total_qty": total })))
}

/// Put a use back (a mistake): the quantity returns to the stock item and the row leaves the Used list.
pub async fn undo(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    need_tab(&u, TAB)?;
    let id = b.get("id").and_then(Value::as_i64).unwrap_or(0);
    let by: Option<String> = sqlx::query_scalar("SELECT CAST(operator AS CHAR) FROM stock_used WHERE id = ? AND voided = 0").bind(id).fetch_optional(&s.db).await?;
    let Some(by) = by else { return Err(AppError::bad("That one was already put back.")) };
    if !u.is_admin() && by != u.name {
        return Err(AppError::Forbidden("Only an admin or the person who used it can put it back.".into()));
    }
    sqlx::query("UPDATE stock_used SET voided = 1, voided_by = ?, voided_at = ? WHERE id = ? AND voided = 0").bind(&u.name).bind(now_minute()).bind(id).execute(&s.db).await?;
    s.bump();
    Ok(Json(json!({ "ok": true })))
}

/// How much of every stock item has been used: {"order:12": 1, "kohls:3": 2}, for the Stocks cards.
pub async fn summary(_u: CurrentUser, State(s): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query("SELECT CAST(source AS CHAR), CAST(ref AS SIGNED), CAST(SUM(qty) AS SIGNED) FROM stock_used WHERE voided = 0 GROUP BY source, ref").fetch_all(&s.db).await?;
    let used: serde_json::Map<String, Value> = rows.iter().map(|r| (format!("{}:{}", text(r, 0), r.try_get_unchecked::<i64, _>(1).unwrap_or(0)), json!(r.try_get_unchecked::<i64, _>(2).unwrap_or(0)))).collect();
    Ok(Json(json!({ "used": used })))
}

#[cfg(test)]
mod tests {
    use super::qty_of;

    #[test]
    fn quantity_of_a_stock_item() {
        assert_eq!(qty_of("2"), 2);
        assert_eq!(qty_of(" 3 "), 3);
        assert_eq!(qty_of("1,200"), 1200);
        assert_eq!(qty_of(""), 1); // blank means one
        assert_eq!(qty_of("abc"), 1);
        assert_eq!(qty_of("0"), 1);
    }
}
