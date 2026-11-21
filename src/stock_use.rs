//! Use Stock: send a product to a customer from the stock that was set aside (returned items kept with a STOCK NO.).
//!
//! A stock item is an order of All Orders with Stock ticked, or a row of Kohl's Returns. Its quantity is its QTY. Using stock takes the
//! quantity straight off that QTY (so the Stocks cards and All Orders show what is left) and writes one row of `stock_used` for the Used
//! list. At 0 the item leaves the Stocks cards. Undoing a use adds the quantity back and marks the row voided (rows are never deleted).

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::Json;
use serde_json::{json, Value};
use sqlx::{MySqlConnection, Row};

use crate::auth::{need_tab, CurrentUser, User};
use crate::error::{AppError, AppResult};
use crate::orders_write::now_minute;
use crate::AppState;

const TAB: &str = "stockuse";

/// One person at a time changes a stock quantity (a single server runs the app), so two people can never take the same unit.
static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn clip(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

fn text(r: &sqlx::mysql::MySqlRow, i: usize) -> String {
    r.try_get_unchecked::<String, _>(i).unwrap_or_default()
}

/// A stock item's quantity: its QTY (0 when it was all used), and 1 when the QTY is blank or not a number.
fn qty_of(v: &str) -> i64 {
    v.trim().replace(',', "").parse::<f64>().map(|f| (f.round() as i64).max(0)).unwrap_or(1)
}

/// What the pages show of one stock item.
#[allow(clippy::too_many_arguments)]
fn shape(source: &str, id: i64, stock_no: String, sku: String, product: String, store: String, image: String, qty: &str) -> Value {
    let n = qty_of(qty);
    json!({ "source": source, "ref": id, "stock_no": stock_no, "sku": sku, "product": product, "store": store, "image": image, "qty": n, "remaining": n })
}

/// One stock item, or None when it is gone or no longer stock.
async fn item(db: &mut MySqlConnection, s: &AppState, source: &str, id: i64) -> AppResult<Option<Value>> {
    if source == "kohls" {
        let row = sqlx::query("SELECT CAST(id AS SIGNED), CAST(stock_no AS CHAR), CAST(sku AS CHAR), CAST(IFNULL(product,'') AS CHAR), CAST(store AS CHAR), CAST(IFNULL(image,'') AS CHAR), CAST(IFNULL(qty,'') AS CHAR) FROM kohls WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
        return Ok(row.map(|r| shape("kohls", id, text(&r, 1), text(&r, 2), text(&r, 3), text(&r, 4), text(&r, 5), &text(&r, 6))));
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
    Ok(Some(shape("order", id, text(&r, 1), text(&r, 2), text(&r, 3), text(&r, 4), text(&r, 5), &text(&r, 6))))
}

/// The quantity now in the QTY cell of a stock item, whether or not it is still stock (None when the row is gone).
async fn current_qty(db: &mut MySqlConnection, s: &AppState, source: &str, id: i64) -> AppResult<Option<i64>> {
    let sql = if source == "kohls" { "SELECT CAST(IFNULL(qty,'') AS CHAR) FROM kohls WHERE id = ?".to_string() } else { format!("SELECT {} FROM orders_text WHERE rn = ?", s.schema().c("QTY")) };
    Ok(sqlx::query(&sql).bind(id).fetch_optional(&mut *db).await?.map(|r| qty_of(&text(&r, 0))))
}

/// Put a new quantity in the QTY of a stock item. For an order this is the same save as typing in All Orders (protections, "Updated by").
async fn write_qty(s: &AppState, u: &User, source: &str, id: i64, qty: i64) -> AppResult<()> {
    if source == "kohls" {
        sqlx::query("UPDATE kohls SET qty = ?, updated_by = ?, updated_at = ? WHERE id = ?").bind(qty).bind(&u.name).bind(now_minute()).bind(id).execute(&s.db).await?;
        s.bump();
        return Ok(());
    }
    let Some(col) = s.schema().at("QTY") else { return Err(AppError::bad("All Orders has no QTY column.")) };
    let Json(out) = crate::orders_write::cells(CurrentUser(u.clone()), State(s.clone()), Json(json!({ "edits": [{ "rn": id, "col": col, "value": qty.to_string() }] }))).await?;
    if out["skipped"].as_i64().unwrap_or(0) > 0 {
        return Err(AppError::bad(out["reason"].as_str().unwrap_or("The QTY cell is protected.").to_string()));
    }
    Ok(())
}

/// Using stock (and putting it back) is for the office; warehouse users only see the Stock History.
fn not_warehouse(u: &User) -> AppResult<()> {
    if u.role == "warehouse" {
        Err(AppError::Forbidden("Using stock is not open to warehouse users.".into()))
    } else {
        Ok(())
    }
}

/// The stock items with this STOCK NO. (and how much of each is left).
pub async fn find(CurrentUser(u): CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    need_tab(&u, "cards")?;
    not_warehouse(&u)?;
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

/// Take `qty` of a stock item: it comes off the QTY.
pub async fn use_stock(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    need_tab(&u, "cards")?;
    not_warehouse(&u)?;
    let source = b.get("source").and_then(Value::as_str).unwrap_or("");
    if source != "order" && source != "kohls" {
        return Err(AppError::bad("Pick the stock item first."));
    }
    let id = b.get("ref").and_then(Value::as_i64).unwrap_or(0);
    let qty = b.get("qty").and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|t| t.trim().parse().ok()))).unwrap_or(0);
    let note = clip(b.get("note").and_then(Value::as_str).unwrap_or(""), 500);
    if qty < 1 {
        return Err(AppError::bad("Enter how many to use (1 or more)."));
    }

    let _one_at_a_time = GATE.lock().await;
    let mut db = s.db.acquire().await?;
    let Some(it) = item(&mut db, &s, source, id).await? else { return Err(AppError::bad("That item is no longer in stock.")) };
    let left = it["remaining"].as_i64().unwrap_or(0);
    if qty > left {
        return Err(AppError::bad(if left == 0 { "There is none of this stock left.".to_string() } else { format!("Only {left} left in stock.") }));
    }
    write_qty(&s, &u, source, id, left - qty).await?;
    sqlx::query("INSERT INTO stock_used (at, operator, source, ref, stock_no, sku, product, qty, order_ref, note) VALUES (?,?,?,?,?,?,?,?,'',?)")
        .bind(now_minute())
        .bind(&u.name)
        .bind(source)
        .bind(id)
        .bind(it["stock_no"].as_str().unwrap_or(""))
        .bind(it["sku"].as_str().unwrap_or(""))
        .bind(it["product"].as_str().unwrap_or(""))
        .bind(qty)
        .bind(&note)
        .execute(&mut *db)
        .await?;
    s.bump();
    Ok(Json(json!({ "item": item(&mut db, &s, source, id).await? })))
}

/// What has been used, newest first.
pub async fn used(CurrentUser(u): CurrentUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> AppResult<Json<Value>> {
    need_tab(&u, TAB)?;
    let word = clip(q.get("q").map(String::as_str).unwrap_or(""), 100).to_lowercase();
    let like = format!("%{}%", word.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query(
        "SELECT CAST(id AS SIGNED), CAST(DATE_FORMAT(at, '%Y-%m-%d %H:%i') AS CHAR), CAST(operator AS CHAR), CAST(source AS CHAR), CAST(stock_no AS CHAR), CAST(sku AS CHAR), CAST(IFNULL(product,'') AS CHAR), \
         CAST(qty AS SIGNED), CAST(order_ref AS CHAR), CAST(IFNULL(note,'') AS CHAR) FROM stock_used WHERE voided = 0 \
         AND (? = '' OR LOWER(CONCAT_WS(' ', stock_no, sku, product, operator, note)) LIKE ?) ORDER BY id DESC LIMIT 500",
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
                    "qty": r.try_get_unchecked::<i64, _>(7).unwrap_or(0), "order_ref": text(r, 8), "note": text(r, 9), "can_undo": u.role != "warehouse" && (u.is_admin() || by == u.name) })
        })
        .collect();
    Ok(Json(json!({ "rows": rows, "total_qty": total })))
}

/// Put a use back (a mistake): the quantity goes back on the stock item's QTY and the row leaves the Used list.
pub async fn undo(CurrentUser(u): CurrentUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    need_tab(&u, TAB)?;
    not_warehouse(&u)?;
    let id = b.get("id").and_then(Value::as_i64).unwrap_or(0);
    let _one_at_a_time = GATE.lock().await;
    let mut db = s.db.acquire().await?;
    let row = sqlx::query("SELECT CAST(operator AS CHAR), CAST(source AS CHAR), CAST(ref AS SIGNED), CAST(qty AS SIGNED) FROM stock_used WHERE id = ? AND voided = 0").bind(id).fetch_optional(&mut *db).await?;
    let Some(row) = row else { return Err(AppError::bad("That one was already put back.")) };
    let (by, source, item_id, qty) = (text(&row, 0), text(&row, 1), row.try_get_unchecked::<i64, _>(2).unwrap_or(0), row.try_get_unchecked::<i64, _>(3).unwrap_or(0));
    if !u.is_admin() && by != u.name {
        return Err(AppError::Forbidden("Only an admin or the person who used it can put it back.".into()));
    }
    let Some(now) = current_qty(&mut db, &s, &source, item_id).await? else { return Err(AppError::bad("That stock item is gone, so the quantity cannot be put back.")) };
    write_qty(&s, &u, &source, item_id, now + qty).await?;
    sqlx::query("UPDATE stock_used SET voided = 1, voided_by = ?, voided_at = ? WHERE id = ? AND voided = 0").bind(&u.name).bind(now_minute()).bind(id).execute(&mut *db).await?;
    s.bump();
    Ok(Json(json!({ "ok": true })))
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
        assert_eq!(qty_of("0"), 0); // all used
        assert_eq!(qty_of("-2"), 0);
    }
}
