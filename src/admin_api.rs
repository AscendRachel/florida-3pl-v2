//! What admins (and people with the rights) do to All Orders beyond typing in cells: dropdown choices, protecting columns and rows,
//! saved filter views, and adding / renaming / deleting / restoring columns.

use std::collections::BTreeSet;

use axum::extract::{Path, State};
use axum::response::Json;
use rand::RngCore;
use serde_json::{json, Map, Value};

use crate::auth::{AdminUser, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::orders_write::{now_minute, recompute_formulas};
use crate::schema::{Schema, SEARCH};
use crate::{auth, settings, AppState};

fn hex_id(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn text(b: &Value, k: &str) -> String {
    b.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn int(b: &Value, k: &str, default: i64) -> i64 {
    match b.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Value::String(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

// ---------------------------------------------------------------- dropdown choices

/// Change the choices of the dropdown on one cell. scope 'list': every cell that shares the list.
/// scope 'column': every dropdown cell in that column gets the new choices.
pub async fn dropdown(_a: AdminUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let (mut options, mut seen) = (Vec::<String>::new(), BTreeSet::new());
    for o in b.get("options").and_then(Value::as_array).into_iter().flatten() {
        let o = o.as_str().map(str::to_string).unwrap_or_else(|| o.to_string()).trim().to_string();
        if !o.is_empty() && seen.insert(o.to_lowercase()) {
            options.push(o);
        }
    }
    if options.is_empty() {
        return Err(AppError::bad("A dropdown needs at least one choice."));
    }
    if options.len() > 200 || options.iter().any(|o| o.chars().count() > 200) {
        return Err(AppError::bad("Too many choices, or a choice is too long."));
    }
    let (rn, col, scope) = (int(&b, "rn", 0), int(&b, "col", -1), b.get("scope").and_then(Value::as_str).unwrap_or("list").to_string());
    if col < 0 || !sch.active(col as usize) {
        return Err(AppError::bad("That cell has no dropdown."));
    }
    let field = sch.cols[col as usize].field.clone();
    let mut tx = s.db.begin().await?;
    let Some(mut lid) = sqlx::query_scalar::<_, u32>("SELECT list_id FROM dropdown_cells WHERE rn = ? AND field = ?").bind(rn).bind(&field).fetch_optional(&mut *tx).await? else {
        return Err(AppError::bad("That cell has no dropdown."));
    };
    let strict: i8 = sqlx::query_scalar("SELECT strict FROM dropdown_lists WHERE id = ?").bind(lid).fetch_one(&mut *tx).await?;
    let json_text = serde_json::to_string(&options)?;
    let cells: i64 = if scope == "column" {
        lid = sqlx::query("INSERT INTO dropdown_lists (options, strict) VALUES (?, ?)").bind(&json_text).bind(strict).execute(&mut *tx).await?.last_insert_id() as u32;
        sqlx::query("UPDATE dropdown_cells SET list_id = ? WHERE field = ?").bind(lid).bind(&field).execute(&mut *tx).await?;
        sqlx::query_scalar("SELECT COUNT(*) FROM dropdown_cells WHERE field = ?").bind(&field).fetch_one(&mut *tx).await?
    } else {
        sqlx::query("UPDATE dropdown_lists SET options = ? WHERE id = ?").bind(&json_text).bind(lid).execute(&mut *tx).await?;
        sqlx::query_scalar("SELECT COUNT(*) FROM dropdown_cells WHERE list_id = ?").bind(lid).fetch_one(&mut *tx).await?
    };
    sqlx::query("DELETE FROM dropdown_lists WHERE id NOT IN (SELECT DISTINCT list_id FROM dropdown_cells)").execute(&mut *tx).await?;
    if scope == "column" {
        sqlx::query("UPDATE column_defs SET options = ? WHERE field = ? AND dropdown = 1").bind(&json_text).bind(&field).execute(&mut *tx).await?; // an added dropdown column: new rows get the new choices
    }
    tx.commit().await?;
    s.bump();
    Ok(Json(json!({ "id": lid, "options": options, "strict": strict != 0, "cells": cells, "scope": scope })))
}

// ---------------------------------------------------------------- protected columns and rows

fn runs(nums: &BTreeSet<u64>) -> Value {
    let mut out: Vec<[u64; 2]> = vec![];
    for &n in nums {
        match out.last_mut() {
            Some(last) if n == last[1] + 1 => last[1] = n,
            _ => out.push([n, n]),
        }
    }
    json!(out)
}

fn expand(ranges: &Value) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    for r in ranges.as_array().into_iter().flatten() {
        let (a, b) = (r[0].as_u64().unwrap_or(1), r[1].as_u64().unwrap_or(0));
        out.extend(a..=b);
    }
    out
}

/// Protect (or, with mode "", unprotect) columns or rows of the orders.
pub async fn locks_set(AdminUser(me): AdminUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let (sheet, kind, mode) = (text(&b, "sheet"), text(&b, "kind"), text(&b, "mode"));
    let targets: Vec<Value> = b.get("targets").and_then(Value::as_array).cloned().unwrap_or_default();
    if targets.is_empty() {
        return Err(AppError::bad("Nothing chosen to protect."));
    }
    if !["orders", "returns"].contains(&sheet.as_str()) || !(kind == "col" || kind == "row") {
        return Err(AppError::bad("That cannot be protected."));
    }
    if !["", "nobody", "admins", "people"].contains(&mode.as_str()) {
        return Err(AppError::bad("Choose who may edit it."));
    }
    let key = |t: &Value| t.as_str().map(str::to_string).unwrap_or_else(|| t.to_string());
    if kind == "col" && sheet == "orders" {
        for t in &targets {
            if key(t).parse::<usize>().ok().is_none_or(|j| !sch.active(j)) {
                return Err(AppError::bad("That column does not exist."));
            }
        }
    } else if kind == "col" {
        let allowed = ["store", "po", "product", "qty", "customer", "address", "return_date", "tracking", "status_cx", "label_link", "due_date", "notes", "status_wh", "refunded", "reason", "shipped", "processed_at", "processed_by", "wh_notes", "rid", "image"];
        if targets.iter().any(|t| !allowed.contains(&key(t).as_str())) {
            return Err(AppError::bad("That column does not exist."));
        }
    }
    let mut d = settings::get(&s.db, "locks").await?.unwrap_or(json!({}));
    for (k, v) in [("defs", json!({})), ("orders", json!({ "cols": {}, "rows": {} })), ("returns", json!({ "cols": {}, "rows": {} }))] {
        if d.get(k).is_none() {
            d[k] = v;
        }
    }
    let new_id = (!mode.is_empty()).then(|| hex_id(5));
    if let Some(id) = &new_id {
        let valid: BTreeSet<String> = auth::list_users(&s.db).await?.iter().filter_map(|u| u["username"].as_str().map(str::to_string)).collect();
        let picked: BTreeSet<String> = if mode == "people" { b.get("users").and_then(Value::as_array).into_iter().flatten().filter_map(|u| u.as_str().map(str::to_string)).filter(|u| valid.contains(u)).collect() } else { BTreeSet::new() };
        let note: String = text(&b, "note").trim().chars().take(200).collect();
        d["defs"][id] = json!({ "mode": mode, "users": picked, "note": note, "by": me.name, "at": now_minute().format("%Y-%m-%d %H:%M").to_string() });
    }
    if kind == "col" {
        for t in &targets {
            let k = key(t);
            if let Some(cols) = d[sheet.as_str()]["cols"].as_object_mut() {
                cols.remove(&k);
                if let Some(id) = &new_id {
                    cols.insert(k, json!(id));
                }
            }
        }
    } else if sheet == "returns" {
        // returns are protected by return id: {lock id: [return id, ...]}
        let chosen: BTreeSet<String> = targets.iter().map(key).collect();
        let ids: Vec<String> = d["returns"]["rows"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
        for lid in ids {
            let left: Vec<Value> = d["returns"]["rows"][&lid].as_array().into_iter().flatten().filter(|r| !chosen.contains(r.as_str().unwrap_or(""))).cloned().collect();
            if left.is_empty() {
                d["returns"]["rows"].as_object_mut().map(|m| m.remove(&lid));
            } else {
                d["returns"]["rows"][&lid] = json!(left);
            }
        }
        if let Some(id) = &new_id {
            d["returns"]["rows"][id] = json!(chosen);
        }
    } else {
        let chosen: BTreeSet<u64> = targets.iter().filter_map(|t| key(t).parse().ok()).collect();
        let ids: Vec<String> = d["orders"]["rows"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
        for lid in ids {
            // take the chosen rows out of whatever protected them before
            let left: BTreeSet<u64> = expand(&d["orders"]["rows"][&lid]).difference(&chosen).copied().collect();
            if left.is_empty() {
                d["orders"]["rows"].as_object_mut().map(|m| m.remove(&lid));
            } else {
                d["orders"]["rows"][&lid] = runs(&left);
            }
        }
        if let Some(id) = &new_id {
            d["orders"]["rows"][id] = runs(&chosen);
        }
    }
    // keep only the definitions that something still uses
    let mut used: BTreeSet<String> = BTreeSet::new();
    for sheet in ["orders", "returns"] {
        used.extend(d[sheet]["cols"].as_object().into_iter().flatten().filter_map(|(_, v)| v.as_str().map(str::to_string)));
        used.extend(d[sheet]["rows"].as_object().into_iter().flatten().map(|(k, _)| k.clone()));
    }
    let defs: Map<String, Value> = d["defs"].as_object().map(|m| m.iter().filter(|(k, _)| used.contains(*k)).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    d["defs"] = Value::Object(defs);
    settings::put(&s.db, "locks", &d).await?;
    s.bump();
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------- saved filter views

/// What a filter view remembers: the search, the summary card, every column filter and the sort.
fn clean_view(b: &Value) -> Value {
    let mut cf = Map::new();
    for (k, f) in b.get("cf").and_then(Value::as_object).into_iter().flatten() {
        if k.parse::<usize>().is_err() || !f.is_object() {
            continue;
        }
        let clip = |v: &Value, n: usize| v.as_str().unwrap_or("").chars().take(n).collect::<String>();
        let cond = f.get("c").filter(|c| c.get("op").and_then(Value::as_str).is_some_and(|o| !o.is_empty())).map(|c| json!({ "op": clip(&c["op"], 20), "a": clip(&c["a"], 200), "b": clip(&c["b"], 200) }));
        let vals = f.get("v").filter(|v| v.is_object()).map(|v| json!({ "mode": if v["mode"] == "in" { "in" } else { "out" }, "list": v["list"].as_array().into_iter().flatten().take(2000).map(|x| clip(x, 200)).collect::<Vec<_>>() }));
        let colors = f.get("k").and_then(|k| k.get("list")).and_then(Value::as_array).filter(|l| !l.is_empty()).map(|l| json!({ "list": l.iter().take(40).map(|x| clip(x, 12)).collect::<Vec<_>>() }));
        if cond.is_some() || vals.is_some() || colors.is_some() {
            cf.insert(k.clone(), json!({ "c": cond, "v": vals, "k": colors }));
        }
    }
    let clip = |k: &str, n: usize| text(b, k).chars().take(n).collect::<String>();
    json!({ "q": clip("q", 200), "card": clip("card", 40), "cf": cf, "sort": clip("sort", 40), "dir": if b.get("dir").and_then(Value::as_str) == Some("desc") { "desc" } else { "asc" } })
}

pub async fn views(CurrentUser(me): CurrentUser, State(s): State<AppState>, Path(action): Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let mut views: Vec<Value> = settings::get(&s.db, "views").await?.and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let vid = text(&b, "id");
    let cur = views.iter().position(|v| v["id"].as_str() == Some(&vid));
    if ["save", "rename", "delete"].contains(&action.as_str()) {
        if let Some(i) = cur {
            if views[i]["owner"].as_str() != Some(&me.username) && !me.is_admin() {
                return Err(AppError::bad("Only the person who made this view, or an admin, can change it."));
            }
        }
    }
    match action.as_str() {
        "delete" => views.retain(|v| v["id"].as_str() != Some(&vid)),
        "save" | "rename" => {
            let given = text(&b, "name");
            let name: String = (if given.is_empty() { cur.map(|i| views[i]["name"].as_str().unwrap_or("").to_string()).unwrap_or_default() } else { given }).trim().chars().take(80).collect();
            if name.is_empty() {
                return Err(AppError::bad("A filter view needs a name."));
            }
            if action == "rename" && cur.is_none() {
                return Err(AppError::bad("That view no longer exists."));
            }
            let mut rec = cur.map(|i| views[i].clone()).unwrap_or_else(|| json!({ "id": hex_id(8), "owner": me.username, "owner_name": me.name }));
            rec["name"] = json!(name);
            if action == "save" {
                if let (Some(r), Some(c)) = (rec.as_object_mut(), clean_view(&b).as_object()) {
                    r.extend(c.clone());
                }
            }
            match cur {
                Some(i) => views[i] = rec,
                None => views.push(rec),
            }
        }
        _ => return Err(AppError::NotFound),
    }
    settings::put(&s.db, "views", &json!(views)).await?;
    Ok(Json(json!({ "views": views })))
}

// ---------------------------------------------------------------- columns

/// The searchable text of every order, rebuilt (after a searched column was deleted or came back).
async fn rebuild_search(s: &AppState, sch: &Schema) -> AppResult<()> {
    let parts: Vec<String> = sch.search_idx().iter().filter(|i| !sch.cols[**i].custom).map(|i| format!("COALESCE(`{}`, '')", sch.cols[*i].field)).collect();
    if parts.is_empty() {
        return Ok(());
    }
    sqlx::query(&format!("UPDATE orders SET search_text = LOWER(CONCAT_WS(' ', {}))", parts.join(", "))).execute(&s.db).await?;
    Ok(())
}

fn key_of(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

pub async fn columns(AdminUser(me): AdminUser, State(s): State<AppState>, Path(action): Path<String>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let sch = s.schema();
    let taken = |name: &str, except: Option<usize>| sch.cols.iter().any(|c| !c.deleted && Some(c.i) != except && (key_of(&c.header) == key_of(name) || key_of(&c.label) == key_of(name)));
    match action.as_str() {
        "add" => {
            let name = text(&b, "name").split_whitespace().collect::<Vec<_>>().join(" ");
            let kind = if b.get("kind").is_some() { text(&b, "kind") } else { "text".into() };
            if !(1..=60).contains(&name.chars().count()) {
                return Err(AppError::bad("Give the column a name (up to 60 characters)."));
            }
            if taken(&name, None) {
                return Err(AppError::bad(format!("There is already a column called \"{name}\".")));
            }
            if !["text", "num", "date", "bool", "dropdown"].contains(&kind.as_str()) {
                return Err(AppError::bad("Pick a column type."));
            }
            let mut options: Vec<String> = vec![];
            if kind == "dropdown" {
                for o in text(&b, "options").lines() {
                    let o = o.trim();
                    if !o.is_empty() && !options.iter().any(|x| x.to_lowercase() == o.to_lowercase()) {
                        options.push(o.chars().take(200).collect());
                    }
                }
                if options.is_empty() {
                    return Err(AppError::bad("A dropdown column needs at least one choice (one per line)."));
                }
            }
            let n = sch.n();
            let field = format!("c_{n}");
            let mut tx = s.db.begin().await?;
            sqlx::query("INSERT INTO column_defs (field, header, label, kind, position, custom, dropdown, options) VALUES (?,?,?,?,?,1,?,?)")
                .bind(&field).bind(&name).bind(&name).bind(if kind == "dropdown" { "text" } else { &kind }).bind(n as i64).bind((kind == "dropdown") as i8).bind((kind == "dropdown").then(|| serde_json::to_string(&options).unwrap_or_default()))
                .execute(&mut *tx).await?;
            if kind == "dropdown" {
                let lid = sqlx::query("INSERT INTO dropdown_lists (options, strict) VALUES (?, 1)").bind(serde_json::to_string(&options)?).execute(&mut *tx).await?.last_insert_id();
                sqlx::query("INSERT IGNORE INTO dropdown_cells (rn, field, list_id) SELECT rn, ?, ? FROM orders").bind(&field).bind(lid).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            s.reload_schema().await?;
            s.bump();
            Ok(Json(json!({ "i": n, "name": name })))
        }
        "rename" => {
            let i = int(&b, "i", -1);
            if i < 0 || !sch.active(i as usize) {
                return Err(AppError::bad("That column does not exist."));
            }
            let i = i as usize;
            let label = text(&b, "label").split_whitespace().collect::<Vec<_>>().join(" ");
            if label.chars().count() > 60 {
                return Err(AppError::bad("The name can be up to 60 characters."));
            }
            let shown = if label.is_empty() { sch.cols[i].header.clone() } else { label };
            if taken(&shown, Some(i)) {
                return Err(AppError::bad(format!("There is already a column called \"{shown}\".")));
            }
            sqlx::query("UPDATE column_defs SET label = ? WHERE id = ?").bind(&shown).bind(sch.cols[i].id).execute(&s.db).await?;
            s.reload_schema().await?;
            s.bump();
            Ok(Json(json!({ "i": i, "name": shown })))
        }
        "delete" => {
            let i = int(&b, "i", -1);
            if i < 0 || !sch.active(i as usize) {
                return Err(AppError::bad("That column does not exist."));
            }
            let i = i as usize;
            let name = sch.cols[i].header.clone();
            let why = sch.protected(&name);
            if !why.is_empty() {
                return Err(AppError::bad(format!("\"{name}\" cannot be deleted. {why}.")));
            }
            let searched = sch.search_idx().contains(&i);
            sqlx::query("UPDATE column_defs SET deleted = 1, header = ?, deleted_by = ?, deleted_at = ? WHERE id = ?").bind(format!("~deleted {i}~ {name}")).bind(&me.name).bind(now_minute()).bind(sch.cols[i].id).execute(&s.db).await?;
            s.reload_schema().await?;
            if searched {
                rebuild_search(&s, &s.schema()).await?; // the deleted text must not stay findable through the search box
            }
            s.bump();
            Ok(Json(json!({ "deleted": name })))
        }
        "restore" => {
            let i = int(&b, "i", -1);
            if i < 0 || (i as usize) >= sch.n() || sch.active(i as usize) {
                return Err(AppError::bad("That column is not in the trash."));
            }
            let i = i as usize;
            let mut name = sch.cols[i].header.split_once("~ ").map(|(_, n)| n.to_string()).unwrap_or_else(|| format!("Column {i}"));
            if taken(&name, None) {
                // a new column took the name meanwhile
                let base = format!("{name} (restored)");
                let mut n = 2;
                name = base.clone();
                while taken(&name, None) {
                    name = format!("{base} {n}");
                    n += 1;
                }
            }
            sqlx::query("UPDATE column_defs SET deleted = 0, header = ?, deleted_by = NULL, deleted_at = NULL WHERE id = ?").bind(&name).bind(sch.cols[i].id).execute(&s.db).await?;
            s.reload_schema().await?;
            let sch = s.schema();
            let mut tx = s.db.begin().await?;
            recompute_formulas(&mut tx, &sch, None).await?;
            tx.commit().await?;
            if SEARCH.contains(&name.as_str()) {
                rebuild_search(&s, &sch).await?; // its text is searchable again
            }
            s.bump();
            Ok(Json(json!({ "restored": name })))
        }
        _ => Err(AppError::NotFound),
    }
}
