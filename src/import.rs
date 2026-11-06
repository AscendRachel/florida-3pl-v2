//! `florida-3pl import <old data folder>`: copies everything the Python app kept (its `data` folder: 8 SQLite files and
//! 4 JSON files) into MariaDB. It only reads the old files. Run it on a COPY of the folder if the old app is still running.
//!
//! Text becomes real types on the way: dates are dates, a checkbox "TRUE"/"FALSE" is 1/0, QTY and counters are numbers,
//! materials amounts are decimals, and blank is NULL. Column numbers (c0, c1 ...) become column names (label_number ...).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{bail, Context, Result};
use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};
use serde_json::Value;
use sqlx::{MySql, MySqlPool, QueryBuilder};

use crate::{auth, clock};

/// Dates and numbers that could not be read (they are stored as NULL and counted, so nothing is lost silently).
static UNREADABLE: AtomicUsize = AtomicUsize::new(0);

pub enum Val {
    Null,
    T(String),
    I(i64),
    F(f64),
    D(NaiveDate),
    Dt(NaiveDateTime),
}

// ---------------------------------------------------------------- converting the old text

fn txt(r: &Row, i: usize) -> Option<String> {
    match r.get_ref(i).ok()? {
        ValueRef::Null | ValueRef::Blob(_) => None,
        ValueRef::Integer(n) => Some(n.to_string()),
        ValueRef::Real(f) => Some(f.to_string()),
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
    }
}

fn blank(s: Option<String>) -> Option<String> {
    s.filter(|t| !t.trim().is_empty())
}

/// Text, NULL when blank.
fn text(s: Option<String>) -> Val {
    blank(s).map_or(Val::Null, Val::T)
}

/// Text for a NOT NULL column: blank is "".
fn req(s: Option<String>) -> Val {
    Val::T(s.unwrap_or_default())
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim().get(..10)?, "%Y-%m-%d").ok()
}

fn parse_dt(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            return Some(d);
        }
    }
    parse_date(s).and_then(|d| d.and_hms_opt(0, 0, 0))
}

fn date(s: Option<String>) -> Val {
    match blank(s) {
        None => Val::Null,
        Some(t) => parse_date(&t).map(Val::D).unwrap_or_else(|| {
            UNREADABLE.fetch_add(1, Ordering::Relaxed);
            Val::Null
        }),
    }
}

fn datetime(s: Option<String>) -> Val {
    match blank(s) {
        None => Val::Null,
        Some(t) => parse_dt(&t).map(Val::Dt).unwrap_or_else(|| {
            UNREADABLE.fetch_add(1, Ordering::Relaxed);
            Val::Null
        }),
    }
}

/// A DATETIME NOT NULL column: when the old value cannot be read, use `fallback`.
fn datetime_or(s: Option<String>, fallback: NaiveDateTime) -> Val {
    Val::Dt(s.as_deref().and_then(parse_dt).unwrap_or(fallback))
}

fn tick(s: Option<String>) -> Val {
    match s.as_deref().map(|t| t.trim().to_ascii_uppercase()).as_deref() {
        Some("TRUE") | Some("1") => Val::I(1),
        Some("FALSE") | Some("0") => Val::I(0),
        _ => Val::Null,
    }
}

/// A checkbox of a NOT NULL column: anything but TRUE is 0.
fn tick0(s: Option<String>) -> Val {
    Val::I(matches!(tick(s), Val::I(1)) as i64)
}

fn number(s: Option<String>) -> Option<f64> {
    let t = blank(s)?;
    match t.replace(',', "").trim().parse::<f64>() {
        Ok(f) if f.is_finite() => Some(f),
        _ => {
            UNREADABLE.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

fn int(s: Option<String>) -> Val {
    number(s).map_or(Val::Null, |f| Val::I(f.round() as i64))
}

fn decimal(s: Option<String>) -> Val {
    number(s).map_or(Val::Null, Val::F)
}

/// The old app's column name as a field name: "SHIPPED TO  CUSTOMER DATE" -> shipped_to_customer_date.
fn ident(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars().filter(char::is_ascii) {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    if out.is_empty() {
        "col".into()
    } else {
        out
    }
}

// ---------------------------------------------------------------- reading and writing

fn open(dir: &Path, file: &str) -> Result<Option<Connection>> {
    let p = dir.join(file);
    if !p.exists() {
        println!("  (no {file}: skipped)");
        return Ok(None);
    }
    Ok(Some(Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY).with_context(|| format!("cannot open {}", p.display()))?))
}

fn rows<F>(c: &Connection, sql: &str, mut f: F) -> Result<Vec<Vec<Val>>>
where
    F: FnMut(&Row) -> Option<Vec<Val>>,
{
    let mut st = c.prepare(sql).with_context(|| format!("reading: {sql}"))?;
    let mut out = vec![];
    let mut it = st.query([])?;
    while let Some(r) = it.next()? {
        if let Some(v) = f(r) {
            out.push(v);
        }
    }
    Ok(out)
}

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap_or(0)
}

async fn insert(db: &MySqlPool, table: &str, cols: &[&str], data: Vec<Vec<Val>>) -> Result<usize> {
    let mut conn = db.acquire().await?;
    insert_conn(&mut conn, table, cols, &data).await
}

/// Writes rows to a table in batches, on the connection (or transaction) it is given.
pub async fn insert_conn(db: &mut sqlx::MySqlConnection, table: &str, cols: &[&str], data: &[Vec<Val>]) -> Result<usize> {
    if data.is_empty() {
        return Ok(0);
    }
    let per = (60000 / cols.len()).clamp(1, 1000);
    let head = format!("INSERT INTO `{table}` ({}) ", cols.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", "));
    for chunk in data.chunks(per) {
        let mut qb: QueryBuilder<MySql> = QueryBuilder::new(&head);
        qb.push_values(chunk, |mut b, row| {
            for v in row {
                match v {
                    Val::Null => b.push_bind(None::<String>),
                    Val::T(s) => b.push_bind(s.as_str()),
                    Val::I(i) => b.push_bind(*i),
                    Val::F(f) => b.push_bind(*f),
                    Val::D(d) => b.push_bind(*d),
                    Val::Dt(d) => b.push_bind(*d),
                };
            }
        });
        qb.build().execute(&mut *db).await.with_context(|| format!("writing to {table}"))?;
    }
    Ok(data.len())
}

fn report(table: &str, from: i64, to: usize) {
    let note = if from == to as i64 { "".to_string() } else { format!("   <-- {} in the old data, {} copied", from, to) };
    println!("  {table:<22} {to:>8}{note}");
}

fn read_json(dir: &Path, file: &str) -> Option<String> {
    let t = std::fs::read_to_string(dir.join(file)).ok()?;
    serde_json::from_str::<Value>(&t).ok().map(|v| v.to_string())
}

const DATA_TABLES: &[&str] = &[
    "order_custom_values", "dropdown_cells", "dropdown_lists", "order_edits", "edit_log", "scans", "ship_events", "boxes", "trash_rows",
    "column_defs", "orders", "import_rejects", "app_settings", "sessions", "users", "work_log_messages", "notifications", "notification_seen", "drive_names",
    "returns", "returns_sync_log", "kohls", "materials", "stock_sorts",
];

// ---------------------------------------------------------------- the import

pub async fn run(db: &MySqlPool, dir: &Path, replace: bool) -> Result<()> {
    if !dir.join("orders.db").exists() && !dir.join("auth.db").exists() {
        bail!("{} does not look like the old app's data folder (no orders.db or auth.db in it)", dir.display());
    }
    let existing: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM orders) + (SELECT COUNT(*) FROM users)").fetch_one(db).await?;
    if existing > 0 && !replace {
        bail!("the database already has data. Run again with --replace to empty it first (everything in it is deleted).");
    }
    if replace {
        println!("Emptying the database first…");
        let mut conn = db.acquire().await?;
        sqlx::query("SET FOREIGN_KEY_CHECKS = 0").execute(&mut *conn).await?;
        for t in DATA_TABLES.iter() {
            sqlx::query(&format!("TRUNCATE TABLE `{t}`")).execute(&mut *conn).await?;
        }
        sqlx::query("SET FOREIGN_KEY_CHECKS = 1").execute(&mut *conn).await?;
    }
    println!("Importing from {}", dir.display());
    println!("  {:<22} {:>8}", "table", "rows");

    users(db, dir).await?;
    let fields = orders(db, dir).await?;
    orders_support(db, dir, &fields).await?;
    settings(db, dir).await?;
    team(db, dir).await?;
    other_tabs(db, dir).await?;

    // the text view of the orders follows the columns that were just imported
    crate::schema::Schema::load(db).await?.make_view(db).await?;

    let bad = UNREADABLE.load(Ordering::Relaxed);
    if bad > 0 {
        println!("\n{bad} date or number values could not be read and were stored as empty.");
    }
    println!("\nDone.");
    Ok(())
}

async fn users(db: &MySqlPool, dir: &Path) -> Result<()> {
    let Some(c) = open(dir, "auth.db")? else { return Ok(()) };
    let now = clock::now();
    let data = rows(&c, "SELECT username, name, pw_hash, salt, role, tabs, disabled, created_at FROM users", |r| {
        let role = txt(r, 4).filter(|x| auth::ROLES.contains(&x.as_str())).unwrap_or_else(|| "member".into());
        let tabs: Vec<String> = txt(r, 5).unwrap_or_default().split(',').map(str::to_string).collect();
        let tabs = if role == "admin" { None } else { auth::parse_tabs(Some(&serde_json::to_string(&tabs).ok()?)).map(|t| serde_json::to_string(&t).unwrap_or_default()) };
        Some(vec![req(txt(r, 0)), req(txt(r, 1)), req(txt(r, 2)), req(txt(r, 3)), Val::T(role), tabs.map_or(Val::Null, Val::T), Val::I(txt(r, 6).and_then(|d| d.parse().ok()).unwrap_or(0)), datetime_or(txt(r, 7), now)])
    })?;
    let n = insert(db, "users", &["username", "name", "pw_hash", "salt", "role", "tabs", "disabled", "created_at"], data).await?;
    report("users", count(&c, "users"), n);
    Ok(())
}

/// Returns, for each old column number, the field name it became (or `c_<n>` for a column an admin added).
async fn orders(db: &MySqlPool, dir: &Path) -> Result<Vec<Option<String>>> {
    let Some(c) = open(dir, "orders.db")? else { return Ok(vec![]) };
    let schema: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("schema.json")).context("schema.json is missing")?)?;
    let entries = schema["columns"].as_array().context("schema.json has no columns")?;

    // the type of every column of the new orders table
    let types: HashMap<String, String> = sqlx::query_as::<_, (String, String)>(
        "SELECT CAST(COLUMN_NAME AS CHAR), CAST(DATA_TYPE AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'orders'",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();
    let reserved = ["rn", "search_text", "created_at", "updated_at"];

    // each old column: a built-in field of the orders table, or a custom column kept in order_custom_values
    let mut field_of: Vec<Option<String>> = vec![];
    let mut defs = vec![];
    let mut seen = HashSet::new();
    for (i, e) in entries.iter().enumerate() {
        let name = e["name"].as_str().unwrap_or("");
        let original = e["was"].as_str().unwrap_or(name);
        let key = ident(original);
        let builtin = types.contains_key(&key) && !reserved.contains(&key.as_str()) && seen.insert(key.clone());
        let field = if builtin { key } else { format!("c_{i}") };
        let kind = e["kind"].as_str().unwrap_or("text").to_string();
        // the label is what an admin renamed the column to (the old schema's "label"); the header never changes
        let label = e["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(name);
        defs.push(vec![
            Val::T(field.clone()),
            Val::T(name.to_string()),
            Val::T(label.to_string()),
            Val::T(kind),
            Val::I(i as i64),
            Val::I(!builtin as i64),
            Val::I(e["deleted"].as_bool().unwrap_or(false) as i64),
            text(e["deleted_by"].as_str().map(str::to_string)),
            datetime(e["deleted_at"].as_str().map(str::to_string)),
            Val::I(e["dropdown"].as_bool().unwrap_or(false) as i64),
            if e["options"].is_array() { Val::T(e["options"].to_string()) } else { Val::Null },
        ]);
        field_of.push(Some(field));
    }
    let n = insert(db, "column_defs", &["field", "header", "label", "kind", "position", "custom", "deleted", "deleted_by", "deleted_at", "dropdown", "options"], defs).await?;
    report("column_defs", entries.len() as i64, n);
    let custom_ids: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>("SELECT CAST(field AS CHAR), CAST(id AS SIGNED) FROM column_defs WHERE custom = 1").fetch_all(db).await?.into_iter().collect();

    let have: HashSet<String> = c.prepare("PRAGMA table_info(orders)")?.query_map([], |r| r.get::<_, String>(1))?.filter_map(|x| x.ok()).collect();
    let n_cols = entries.len();
    if (0..n_cols).any(|i| !have.contains(&format!("c{i}"))) {
        bail!("orders.db does not have the columns schema.json lists (c0 .. c{})", n_cols - 1);
    }

    let builtin: Vec<(usize, String, String)> = field_of
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.as_ref().filter(|f| types.contains_key(*f)).map(|f| (i, f.clone(), types[f].clone())))
        .collect();
    let mut cols: Vec<&str> = vec!["rn"];
    cols.extend(builtin.iter().map(|(_, f, _)| f.as_str()));
    cols.push("search_text");

    let sql = format!("SELECT rn, {}, _s FROM orders ORDER BY rn", (0..n_cols).map(|i| format!("c{i}")).collect::<Vec<_>>().join(", "));
    let mut custom_rows: Vec<Vec<Val>> = vec![];
    let mut rejects: Vec<Vec<Val>> = vec![];
    let data = rows(&c, &sql, |r| {
        let rn: i64 = r.get(0).ok()?;
        let mut v = vec![Val::I(rn)];
        for (i, field, ty) in &builtin {
            let s = txt(r, i + 1);
            let original = blank(s.clone());
            let val = match ty.as_str() {
                "tinyint" => tick(s),
                "int" => int(s),
                "date" => date(s),
                "datetime" => datetime(s),
                _ => text(s),
            };
            if let (Val::Null, Some(o)) = (&val, original) {
                // something was there but does not fit the type: keep the original text in import_rejects
                rejects.push(vec![Val::T("orders".into()), Val::T(rn.to_string()), Val::T(field.clone()), Val::T(o)]);
            }
            v.push(val);
        }
        v.push(Val::T(txt(r, n_cols + 1).unwrap_or_default()));
        for (i, f) in field_of.iter().enumerate() {
            if let Some(id) = f.as_ref().and_then(|f| custom_ids.get(f)) {
                if let Some(s) = blank(txt(r, i + 1)) {
                    custom_rows.push(vec![Val::I(rn), Val::I(*id), Val::T(s)]);
                }
            }
        }
        Some(v)
    })?;
    let n = insert(db, "orders", &cols, data).await?;
    report("orders", count(&c, "orders"), n);
    let n = insert(db, "import_rejects", &["source_table", "row_ref", "field", "value"], rejects).await?;
    if n > 0 {
        println!("  {:<22} {:>8}   <-- values that did not fit their column; the original text is kept here", "import_rejects", n);
    }
    let n = insert(db, "order_custom_values", &["rn", "column_id", "value"], custom_rows).await?;
    if n > 0 {
        report("order_custom_values", n as i64, n);
    }
    Ok(field_of)
}

async fn orders_support(db: &MySqlPool, dir: &Path, fields: &[Option<String>]) -> Result<()> {
    let Some(c) = open(dir, "orders.db")? else { return Ok(()) };
    let field = |col: i64| fields.get(col as usize).cloned().flatten();
    let known: HashSet<i64> = sqlx::query_scalar::<_, i64>("SELECT CAST(rn AS SIGNED) FROM orders").fetch_all(db).await?.into_iter().collect();
    let now = clock::now();

    let data = rows(&c, "SELECT rn, col, value, at FROM edits", |r| {
        let (rn, col): (i64, i64) = (r.get(0).ok()?, r.get(1).ok()?);
        Some(vec![Val::I(rn), Val::T(field(col)?), text(txt(r, 2)), datetime_or(txt(r, 3), now)]).filter(|_| known.contains(&rn))
    })?;
    let n = insert(db, "order_edits", &["rn", "field", "value", "at"], data).await?;
    report("order_edits", count(&c, "edits"), n);

    let data = rows(&c, "SELECT at, who, rn, col, old, new, note FROM edit_log ORDER BY id", |r| {
        let col: i64 = r.get(3).ok()?;
        Some(vec![datetime_or(txt(r, 0), now), req(txt(r, 1)), Val::I(r.get(2).ok()?), Val::T(field(col)?), text(txt(r, 4)), text(txt(r, 5)), req(txt(r, 6))])
    })?;
    let n = insert(db, "edit_log", &["at", "who", "rn", "field", "old_value", "new_value", "note"], data).await?;
    report("edit_log", count(&c, "edit_log"), n);

    let data = rows(&c, "SELECT id, options, strict FROM lists", |r| Some(vec![Val::I(r.get(0).ok()?), Val::T(txt(r, 1).unwrap_or_else(|| "[]".into())), Val::I(txt(r, 2).and_then(|s| s.parse().ok()).unwrap_or(0))]))?;
    let list_ids: HashSet<i64> = data.iter().filter_map(|v| if let Val::I(i) = v[0] { Some(i) } else { None }).collect();
    let n = insert(db, "dropdown_lists", &["id", "options", "strict"], data).await?;
    report("dropdown_lists", count(&c, "lists"), n);

    let data = rows(&c, "SELECT rn, col, list_id FROM dv", |r| {
        let (rn, col, list): (i64, i64, i64) = (r.get(0).ok()?, r.get(1).ok()?, r.get(2).ok()?);
        Some(vec![Val::I(rn), Val::T(field(col)?), Val::I(list)]).filter(|_| known.contains(&rn) && list_ids.contains(&list))
    })?;
    let n = insert(db, "dropdown_cells", &["rn", "field", "list_id"], data).await?;
    report("dropdown_cells", count(&c, "dv"), n);

    let data = rows(&c, "SELECT id, at, operator, code, rn, outcome, note FROM scans ORDER BY id", |r| {
        Some(vec![Val::I(r.get(0).ok()?), datetime_or(txt(r, 1), now), req(txt(r, 2)), req(txt(r, 3)), int(txt(r, 4)), req(txt(r, 5)), text(txt(r, 6))])
    })?;
    let n = insert(db, "scans", &["id", "at", "operator", "code", "rn", "outcome", "note"], data).await?;
    report("scans", count(&c, "scans"), n);

    let data = rows(&c, "SELECT id, rn, kind, at, operator, note FROM ship_events ORDER BY id", |r| {
        let kind = txt(r, 2).filter(|k| k == "customer" || k == "amazon")?;
        Some(vec![Val::I(r.get(0).ok()?), Val::I(r.get(1).ok()?), Val::T(kind), datetime_or(txt(r, 3), now), req(txt(r, 4)), req(txt(r, 5))])
    })?;
    let n = insert(db, "ship_events", &["id", "rn", "kind", "at", "operator", "note"], data).await?;
    report("ship_events", count(&c, "ship_events"), n);

    let data = rows(&c, "SELECT rn, code, at FROM boxes", |r| Some(vec![Val::I(r.get(0).ok()?), req(txt(r, 1)), datetime_or(txt(r, 2), now)]))?;
    let n = insert(db, "boxes", &["rn", "code", "at"], data).await?;
    report("boxes", count(&c, "boxes"), n);

    let data = rows(&c, "SELECT rn, label, product, customer, data, by, at FROM trash_rows", |r| {
        Some(vec![Val::I(r.get(0).ok()?), text(txt(r, 1)), text(txt(r, 2)), text(txt(r, 3)), Val::T(txt(r, 4).unwrap_or_else(|| "{}".into())), req(txt(r, 5)), datetime_or(txt(r, 6), now)])
    })?;
    let n = insert(db, "trash_rows", &["rn", "label", "product", "customer", "data", "deleted_by", "deleted_at"], data).await?;
    report("trash_rows", count(&c, "trash_rows"), n);

    // the last import's note (meta) goes with the other settings
    let mut data = rows(&c, "SELECT k, v FROM meta", |r| {
        let v = txt(r, 1).unwrap_or_default();
        let json = serde_json::from_str::<Value>(&v).map(|j| j.to_string()).unwrap_or_else(|_| Value::String(v).to_string());
        Some(vec![Val::T(format!("orders.{}", txt(r, 0)?)), Val::T(json)])
    })?;
    data.retain(|_| true);
    insert(db, "app_settings", &["k", "v"], data).await?;
    Ok(())
}

async fn settings(db: &MySqlPool, dir: &Path) -> Result<()> {
    let mut data = vec![];
    for (file, key) in [("branding.json", "branding"), ("dashboard.json", "dashboard"), ("locks.json", "locks"), ("views.json", "views"), ("return_labels.json", "return_labels")] {
        if let Some(v) = read_json(dir, file) {
            data.push(vec![Val::T(key.into()), Val::T(v)]);
        }
    }
    let n = insert(db, "app_settings", &["k", "v"], data).await?;
    report("app_settings (json)", n as i64, n);
    Ok(())
}

async fn team(db: &MySqlPool, dir: &Path) -> Result<()> {
    let now = clock::now();
    if let Some(c) = open(dir, "worklog.db")? {
        let data = rows(&c, "SELECT id, at, username, name, text FROM messages ORDER BY id", |r| {
            Some(vec![Val::I(r.get(0).ok()?), datetime_or(txt(r, 1), now), req(txt(r, 2)), req(txt(r, 3)), req(txt(r, 4))])
        })?;
        let n = insert(db, "work_log_messages", &["id", "at", "username", "name", "text"], data).await?;
        report("work_log_messages", count(&c, "messages"), n);
    }
    if let Some(c) = open(dir, "notify.db")? {
        let data = rows(&c, "SELECT id, at, by_user, by_name, by_role, rn, label, customer, field, text, kind, to_user, to_name FROM events ORDER BY id", |r| {
            let kind = txt(r, 10).filter(|k| k == "reply" || k == "rewrite").unwrap_or_default();
            Some(vec![
                Val::I(r.get(0).ok()?), datetime_or(txt(r, 1), now), req(txt(r, 2)), req(txt(r, 3)), req(txt(r, 4)), Val::I(r.get(5).ok()?), req(txt(r, 6)), req(txt(r, 7)),
                Val::T(txt(r, 8).unwrap_or_else(|| "Notes".into())), req(txt(r, 9)), Val::T(kind), req(txt(r, 11)), req(txt(r, 12)),
            ])
        })?;
        let cols = ["id", "at", "by_user", "by_name", "by_role", "rn", "label", "customer", "field", "text", "kind", "to_user", "to_name"];
        let n = insert(db, "notifications", &cols, data).await?;
        report("notifications", count(&c, "events"), n);
        let data = rows(&c, "SELECT username, last_id FROM seen", |r| Some(vec![req(txt(r, 0)), Val::I(r.get(1).ok()?)]))?;
        let n = insert(db, "notification_seen", &["username", "last_id"], data).await?;
        report("notification_seen", count(&c, "seen"), n);
    }
    if let Some(c) = open(dir, "drive.db")? {
        let data = rows(&c, "SELECT id, name, at FROM drive_names", |r| {
            let at: f64 = r.get(2).unwrap_or(0.0);
            Some(vec![req(txt(r, 0)), req(txt(r, 1)), Val::Dt(clock::from_unix(at))])
        })?;
        let n = insert(db, "drive_names", &["id", "name", "read_at"], data).await?;
        report("drive_names", count(&c, "drive_names"), n);
    }
    Ok(())
}

async fn other_tabs(db: &MySqlPool, dir: &Path) -> Result<()> {
    let now = clock::now();
    if let Some(c) = open(dir, "returns.db")? {
        let cols = [
            "id", "rid", "return_key", "store", "po", "product", "qty", "customer", "address", "return_date", "tracking", "status_cx", "label_link", "due_date", "notes",
            "status_wh", "refunded", "reason", "shipped", "processed_at", "processed_by", "wh_notes", "removed", "first_seen", "last_seen", "ovr",
        ];
        let data = rows(
            &c,
            "SELECT id, rid, \"key\", store, po, product, qty, customer, address, return_date, tracking, status_cx, label_link, due_date, notes, status_wh, refunded, reason, \
             shipped, processed_at, processed_by, wh_notes, removed, first_seen, last_seen, ovr FROM returns ORDER BY id",
            |r| {
                let ovr = blank(txt(r, 25)).filter(|t| serde_json::from_str::<Value>(t).is_ok());
                Some(vec![
                    Val::I(r.get(0).ok()?), req(txt(r, 1)), req(txt(r, 2)), req(txt(r, 3)), req(txt(r, 4)), text(txt(r, 5)), int(txt(r, 6)), req(txt(r, 7)), text(txt(r, 8)),
                    date(txt(r, 9)), req(txt(r, 10)), req(txt(r, 11)), text(txt(r, 12)), date(txt(r, 13)), text(txt(r, 14)), req(txt(r, 15)), req(txt(r, 16)), req(txt(r, 17)),
                    tick0(txt(r, 18)), datetime(txt(r, 19)), req(txt(r, 20)), text(txt(r, 21)), Val::I(txt(r, 22).and_then(|s| s.parse().ok()).unwrap_or(0)),
                    datetime_or(txt(r, 23), now), datetime_or(txt(r, 24), now), ovr.map_or(Val::Null, Val::T),
                ])
            },
        )?;
        let n = insert(db, "returns", &cols, data).await?;
        report("returns", count(&c, "returns"), n);

        let data = rows(&c, "SELECT id, at, ok, trigger, message, read, added, updated, removed FROM sync_log ORDER BY id", |r| {
            let num = |i| Val::I(txt(r, i).and_then(|s| s.parse().ok()).unwrap_or(0));
            Some(vec![Val::I(r.get(0).ok()?), datetime_or(txt(r, 1), now), num(2), req(txt(r, 3)), text(txt(r, 4)), num(5), num(6), num(7), num(8)])
        })?;
        let n = insert(db, "returns_sync_log", &["id", "at", "ok", "trigger_by", "message", "read_count", "added", "updated", "removed"], data).await?;
        report("returns_sync_log", count(&c, "sync_log"), n);

        let data = rows(&c, "SELECT id, store, stock_no, sku, order_number, product, image, qty, good, used, notes, updated_by, updated_at FROM kohls ORDER BY id", |r| {
            Some(vec![
                Val::I(r.get(0).ok()?), req(txt(r, 1)), req(txt(r, 2)), req(txt(r, 3)), req(txt(r, 4)), text(txt(r, 5)), text(txt(r, 6)), int(txt(r, 7)), tick0(txt(r, 8)), tick0(txt(r, 9)),
                text(txt(r, 10)), req(txt(r, 11)), datetime(txt(r, 12)),
            ])
        })?;
        let cols = ["id", "store", "stock_no", "sku", "order_number", "product", "image", "qty", "good", "used", "notes", "updated_by", "updated_at"];
        let n = insert(db, "kohls", &cols, data).await?;
        report("kohls", count(&c, "kohls"), n);

        let data = rows(&c, "SELECT id, at, operator, source, ref, stock_no FROM stock_sorts ORDER BY id", |r| {
            Some(vec![Val::I(r.get(0).ok()?), datetime_or(txt(r, 1), now), req(txt(r, 2)), req(txt(r, 3)), req(txt(r, 4)), req(txt(r, 5))])
        })?;
        let n = insert(db, "stock_sorts", &["id", "at", "operator", "source", "ref", "stock_no"], data).await?;
        report("stock_sorts", count(&c, "stock_sorts"), n);
    }
    if let Some(c) = open(dir, "materials.db")? {
        let data = rows(&c, "SELECT id, mid, image, name, spec, unit, pack, ordered, current, alert, minimum, status, updated_at, updated_by, notes FROM materials ORDER BY id", |r| {
            Some(vec![
                Val::I(r.get(0).ok()?), req(txt(r, 1)), text(txt(r, 2)), req(txt(r, 3)), req(txt(r, 4)), req(txt(r, 5)), req(txt(r, 6)), decimal(txt(r, 7)), decimal(txt(r, 8)),
                decimal(txt(r, 9)), decimal(txt(r, 10)), req(txt(r, 11)), datetime(txt(r, 12)), req(txt(r, 13)), text(txt(r, 14)),
            ])
        })?;
        let cols = ["id", "mid", "image", "name", "spec", "unit", "pack", "ordered", "current", "alert", "minimum", "status", "updated_at", "updated_by", "notes"];
        let n = insert(db, "materials", &cols, data).await?;
        report("materials", count(&c, "materials"), n);
    }
    Ok(())
}
