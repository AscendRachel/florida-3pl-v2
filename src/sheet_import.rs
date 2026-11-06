//! The Import tab: upload the downloaded 3PL sheet (.xlsx or .csv) or give a Google Sheets link, pick the orders tab, then replace or
//! add to the orders, or only reload the dropdown lists. Long steps report their progress under a token the page watches.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Json;
use regex::Regex;
use serde_json::{json, Value};
use sqlx::Row;

use crate::amazon::{percent_decode, plain_number};
use crate::auth::AdminUser;
use crate::error::{AppError, AppResult};
use crate::import::{insert_conn, Val};
use crate::orders_write::{norm_date, recompute_formulas, typed, Typed};
use crate::schema::Schema;
use crate::xlsx::{col_index, Workbook};
use crate::{clock, AppState};

pub const MAX_UPLOAD: usize = 300 * 1024 * 1024;

pub type Uploads = std::sync::Arc<std::sync::Mutex<HashMap<String, (PathBuf, String)>>>;
pub type Progress = std::sync::Arc<std::sync::Mutex<HashMap<String, Value>>>;

fn set_progress(s: &AppState, token: &str, pct: Option<f64>, text: &str) {
    if token.is_empty() {
        return;
    }
    let mut p = s.progress.lock().unwrap();
    p.insert(token.to_string(), json!({ "pct": pct.map(|x| (x * 10.0).round() / 10.0), "text": text }));
    while p.len() > 40 {
        let k = p.keys().next().cloned();
        if let Some(k) = k {
            p.remove(&k);
        }
    }
}

pub async fn progress(_a: AdminUser, State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    Json(s.progress.lock().unwrap().get(q.get("token").map(String::as_str).unwrap_or("")).cloned().unwrap_or(json!({})))
}

// ---------------------------------------------------------------- matching the file's columns to ours

/// Some exports have UTF-8 text that was read as Windows-1252 (e.g. 'â€“').
fn fix_text(s: &str) -> String {
    if s.chars().any(|c| "âðÃ".contains(c)) {
        let (bytes, _, bad) = encoding_rs::WINDOWS_1252.encode(s);
        if !bad {
            if let Ok(t) = String::from_utf8(bytes.into_owned()) {
                return t;
            }
        }
    }
    s.to_string()
}

/// Header cells match by name ignoring case, spacing and the emoji / encoding of the Softr column.
fn key_of(header: &str) -> String {
    let k = fix_text(header).split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    if k.contains("softr") {
        "softr record id".into()
    } else {
        k
    }
}

fn column_by_key(sch: &Schema) -> HashMap<String, usize> {
    let mut m: HashMap<String, usize> = sch.cols.iter().filter(|c| !c.deleted).map(|c| (key_of(&c.header), c.i)).collect();
    // a renamed column also answers to its new name (an import file may use either)
    for c in sch.cols.iter().filter(|c| !c.deleted && c.label != c.header) {
        m.entry(key_of(&c.label)).or_insert(c.i);
    }
    m
}

/// First row (within the first 15) that contains a "Label Number" cell -> (row number, header keys).
fn find_header(rows: &[(usize, Vec<String>)]) -> Option<(usize, Vec<String>)> {
    for (n, r) in rows {
        if *n > 15 {
            return None;
        }
        let keys: Vec<String> = r.iter().map(|c| key_of(c)).collect();
        if keys.iter().any(|k| k == "label number") {
            return Some((*n, keys));
        }
    }
    None
}

fn read_csv(path: &std::path::Path, mut tick: Option<&mut dyn FnMut(f64)>) -> anyhow::Result<Vec<(usize, Vec<String>)>> {
    let data = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&data);
    let text = text.trim_start_matches('\u{feff}');
    let size = text.len().max(1);
    let mut rd = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(text.as_bytes());
    let (mut out, mut read) = (vec![], 0usize);
    for (i, r) in rd.records().enumerate() {
        let r = r?;
        let n = i + 1;
        read += r.iter().map(|c| c.len() + 1).sum::<usize>();
        if r.iter().any(|c| !c.trim().is_empty()) {
            out.push((n, r.iter().map(str::to_string).collect()));
        }
        if n % 500 == 0 {
            if let Some(t) = tick.as_mut() {
                t((read as f64 / size as f64).min(1.0));
            }
        }
    }
    Ok(out)
}

fn is_csv(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("csv"))
}

/// Which tabs of the file look like the orders tab: [{name, ok, matched}].
fn preview(path: &std::path::Path, by_key: &HashMap<String, usize>) -> anyhow::Result<Vec<Value>> {
    if is_csv(path) {
        let found = find_header(&read_csv(path, None)?.into_iter().take(15).collect::<Vec<_>>());
        let matched = found.as_ref().map_or(0, |(_, k)| k.iter().filter(|x| by_key.contains_key(*x)).count());
        return Ok(vec![json!({ "name": "CSV file", "ok": found.is_some(), "matched": matched })]);
    }
    let mut wb = Workbook::new(std::fs::File::open(path)?)?;
    let mut out = vec![];
    for (name, target) in wb.sheets()? {
        let found = find_header(&wb.rows(&target, Some(15), None)?);
        let matched = found.as_ref().map_or(0, |(_, k)| k.iter().filter(|x| by_key.contains_key(*x)).count());
        out.push(json!({ "name": name, "ok": found.is_some(), "matched": matched }));
    }
    Ok(out)
}

struct Dropdown {
    options: Vec<String>,
    strict: bool,
    /// (first column, last column, first row, last row) in the sheet, columns from 0.
    cells: Vec<(usize, usize, usize, usize)>,
}

/// The sheet's list validations. Only lists typed into the rule ("Yes,No,Maybe") are supported; lists that point at a range of cells are skipped.
fn parse_dropdowns(rules: &[crate::xlsx::Validation]) -> (Vec<Dropdown>, usize) {
    let (mut out, mut skipped) = (vec![], 0);
    let cell_re = Regex::new(r"^([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?$").unwrap();
    for d in rules.iter().filter(|d| d.kind == "list") {
        let f = d.formula.trim();
        if !(f.len() >= 2 && f.starts_with('"') && f.ends_with('"')) {
            skipped += 1;
            continue;
        }
        let options: Vec<String> = f[1..f.len() - 1].replace("\"\"", "\"").split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
        let mut cells = vec![];
        for tok in d.sqref.split_whitespace() {
            if let Some(m) = cell_re.captures(tok) {
                let c1 = col_index(&m[1]);
                let c2 = m.get(3).map_or(c1, |x| col_index(x.as_str()));
                let (r1, r2) = (m[2].parse().unwrap_or(0), m.get(4).map_or_else(|| m[2].parse().unwrap_or(0), |x| x.as_str().parse().unwrap_or(0)));
                cells.push((c1, c2, r1, r2));
            }
        }
        if !options.is_empty() && !cells.is_empty() {
            out.push(Dropdown { options, strict: d.strict, cells });
        }
    }
    (out, skipped)
}

struct Parsed {
    rows: Vec<(usize, Vec<String>)>,
    matched: usize,
    ignored: Vec<String>,
    missing: Vec<String>,
    skipped: usize,
    dropdowns_unsupported: usize,
    dropdowns: Vec<Dropdown>,
    pos: HashMap<usize, usize>,
}

/// Read the orders out of a sheet -> rows aligned to the columns (as text) keyed by their row number, and the report.
fn parse_orders(path: &std::path::Path, sheet: &str, sch: &Schema, mut tick: Option<&mut dyn FnMut(f64)>) -> AppResult<Parsed> {
    let by_key = column_by_key(sch);
    let (raw, rules) = if is_csv(path) {
        (read_csv(path, tick.take()).map_err(|e| AppError::bad(e.to_string()))?, vec![])
    } else {
        let mut wb = Workbook::new(std::fs::File::open(path).map_err(anyhow::Error::from)?).map_err(|e| AppError::bad(e.to_string()))?;
        let target = wb.sheets().map_err(|e| AppError::bad(e.to_string()))?.into_iter().find(|(n, _)| n == sheet).map(|(_, t)| t).ok_or_else(|| AppError::bad(format!("The tab \"{sheet}\" is not in the file.")))?;
        let rows = wb.rows(&target, None, tick.take()).map_err(|e| AppError::bad(e.to_string()))?;
        (rows, std::mem::take(&mut wb.validations))
    };
    let Some((header_row, keys)) = find_header(&raw.iter().take(40).cloned().collect::<Vec<_>>()) else {
        return Err(AppError::bad(format!("The tab \"{sheet}\" has no header row with a \"Label Number\" column in its first 15 rows.")));
    };
    let mut pos: HashMap<usize, usize> = HashMap::new(); // file column -> our column
    for (fc, k) in keys.iter().enumerate() {
        if let Some(&oc) = by_key.get(k) {
            if !pos.values().any(|v| *v == oc) {
                pos.insert(fc, oc);
            }
        }
    }
    let ignored: Vec<String> = keys.iter().filter(|k| !k.is_empty() && !by_key.contains_key(*k)).cloned().collect();
    let (mut out, mut skipped) = (vec![], 0);
    for (n, r) in raw.into_iter().filter(|(n, _)| *n > header_row) {
        let mut vals = vec![String::new(); sch.n()];
        for (fc, oc) in &pos {
            if let Some(cell) = r.get(*fc) {
                vals[*oc] = plain_number(fix_text(cell).trim());
            }
        }
        if vals.iter().all(String::is_empty) || vals[0] == "Counters" {
            skipped += 1;
            continue;
        }
        for (j, c) in sch.cols.iter().enumerate() {
            if c.kind == "date" && !vals[j].is_empty() {
                vals[j] = norm_date(&vals[j]);
            } else if c.kind == "bool" {
                let up = vals[j].to_uppercase();
                vals[j] = if up == "TRUE" || up == "FALSE" { up } else { String::new() };
            }
        }
        out.push((n, vals));
    }
    let used: HashSet<usize> = pos.values().copied().collect();
    let missing: Vec<String> = sch.cols.iter().filter(|c| !c.deleted && !c.custom && !used.contains(&c.i)).map(|c| c.header.clone()).collect();
    let (dropdowns, unsupported) = parse_dropdowns(&rules);
    Ok(Parsed { rows: out, matched: pos.len(), ignored, missing, skipped, dropdowns_unsupported: unsupported, dropdowns, pos })
}

// ---------------------------------------------------------------- upload

fn pick(sheets: &[Value]) -> Option<String> {
    let ok: Vec<&Value> = sheets.iter().filter(|s| s["ok"].as_bool().unwrap_or(false)).collect();
    ok.iter().find(|s| s["name"].as_str().is_some_and(|n| n.to_lowercase() == "march")).or(ok.first()).and_then(|s| s["name"].as_str().map(str::to_string))
}

async fn save_upload(s: &AppState, data: Vec<u8>, name: &str, token: &str) -> AppResult<Value> {
    set_progress(s, token, None, "Checking the tabs in the file…");
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    if ext != "xlsx" && ext != "csv" {
        return Err(AppError::bad("Upload an Excel .xlsx file or a .csv file. (In Google Sheets: File > Download > Microsoft Excel.)"));
    }
    let mut b = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut b);
    let id = hex::encode(b);
    let dir = PathBuf::from(&s.config.data_dir).join("uploads");
    std::fs::create_dir_all(&dir).map_err(anyhow::Error::from)?;
    let path = dir.join(format!("{id}.{ext}"));
    std::fs::write(&path, &data).map_err(anyhow::Error::from)?;
    let by_key = column_by_key(&s.schema());
    let p2 = path.clone();
    let sheets = tokio::task::spawn_blocking(move || preview(&p2, &by_key)).await?;
    let sheets = match sheets {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(AppError::bad(format!("Could not read that file as .{ext}: {e}")));
        }
    };
    s.uploads.lock().unwrap().insert(id.clone(), (path, name.to_string()));
    Ok(json!({ "id": id, "file": name, "sheets": sheets, "pick": pick(&sheets) }))
}

/// Save the uploaded file (raw request body) and report which tabs look like the orders tab.
pub async fn upload(_a: AdminUser, State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Value>> {
    let name = percent_decode(headers.get("X-Filename").and_then(|v| v.to_str().ok()).unwrap_or("upload"));
    if body.is_empty() {
        return Err(AppError::bad("The file is empty."));
    }
    let token = headers.get("X-Progress").and_then(|v| v.to_str().ok()).unwrap_or("").chars().take(40).collect::<String>();
    Ok(Json(save_upload(&s, body.to_vec(), &name, &token).await?))
}

/// Download a Google Sheet as .xlsx from its link (the sheet must be shared "Anyone with the link can view").
pub async fn upload_link(_a: AdminUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let url = b.get("url").and_then(Value::as_str).unwrap_or("");
    let Some(m) = Regex::new(r"docs\.google\.com/spreadsheets/d/([A-Za-z0-9_-]{20,})").unwrap().captures(url) else {
        return Err(AppError::bad("That doesn't look like a Google Sheets link. It should start with https://docs.google.com/spreadsheets/d/…"));
    };
    let token: String = b.get("token").and_then(Value::as_str).unwrap_or("").chars().take(40).collect();
    set_progress(&s, &token, None, "Connecting to Google Sheets…");
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(180)).user_agent("Florida3PL").build()?;
    let mut resp = client.get(format!("https://docs.google.com/spreadsheets/d/{}/export?format=xlsx", &m[1])).send().await.map_err(|e| AppError::bad(format!("Could not reach Google Sheets: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::bad(format!("Google refused the download (error {}). Share the sheet as \"Anyone with the link can view\" and try again.", resp.status().as_u16())));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut data: Vec<u8> = vec![];
    while let Some(chunk) = resp.chunk().await.map_err(|e| AppError::bad(format!("Could not reach Google Sheets: {e}")))? {
        data.extend_from_slice(&chunk);
        if data.len() > MAX_UPLOAD {
            return Err(AppError::bad("That sheet is too large (limit 300 MB)."));
        }
        // Google usually does not say the size of an export, so then the bar only shows how much has arrived
        let mb = data.len() as f64 / 1_048_576.0;
        set_progress(&s, &token, (total > 0).then(|| data.len() as f64 / total as f64 * 100.0), &format!("Downloading from Google… {mb:.1} MB{}", if total > 0 { format!(" of {:.1} MB", total as f64 / 1_048_576.0) } else { String::new() }));
    }
    if !data.starts_with(b"PK") {
        return Err(AppError::bad("Google did not return a spreadsheet. Share the sheet as \"Anyone with the link can view\" and try again."));
    }
    Ok(Json(save_upload(&s, data, &format!("Google Sheet ({}…).xlsx", &m[1][..8]), &token).await?))
}

// ---------------------------------------------------------------- the import

fn to_val(t: Typed) -> Val {
    match t {
        Typed::Null => Val::Null,
        Typed::T(s) => Val::T(s),
        Typed::I(i) => Val::I(i),
        Typed::D(d) => Val::D(d),
        Typed::Dt(d) => Val::Dt(d),
    }
}

/// Orders (typed), the values of added columns, and the search text, for the parsed rows. `rn_of` maps the sheet's row number to the order number.
async fn insert_orders(conn: &mut sqlx::MySqlConnection, sch: &Schema, rows: &[(usize, Vec<String>)], rn_of: &HashMap<usize, u64>, tick: &mut (impl FnMut(f64) + Send)) -> anyhow::Result<()> {
    let builtin: Vec<&crate::schema::Col> = sch.cols.iter().filter(|c| !c.custom).collect();
    let mut cols: Vec<&str> = vec!["rn"];
    cols.extend(builtin.iter().map(|c| c.field.as_str()));
    cols.push("search_text");
    let search_idx = sch.search_idx();
    let (mut main, mut custom): (Vec<Vec<Val>>, Vec<Vec<Val>>) = (vec![], vec![]);
    for (n, vals) in rows {
        let rn = rn_of[n];
        let mut r = vec![Val::I(rn as i64)];
        r.extend(builtin.iter().map(|c| to_val(typed(c, &vals[c.i]))));
        r.push(Val::T(search_idx.iter().map(|j| vals[*j].as_str()).collect::<Vec<_>>().join(" ").to_lowercase()));
        main.push(r);
        for c in sch.cols.iter().filter(|c| c.custom && !c.deleted && !vals[c.i].is_empty()) {
            custom.push(vec![Val::I(rn as i64), Val::I(c.id as i64), Val::T(vals[c.i].clone())]);
        }
    }
    let total = main.len().max(1);
    for (i, chunk) in main.chunks(2000).enumerate() {
        insert_conn(conn, "orders", &cols, chunk).await?;
        tick(((i + 1) * 2000).min(total) as f64 / total as f64);
    }
    insert_conn(conn, "order_custom_values", &["rn", "column_id", "value"], &custom).await?;
    Ok(())
}

/// Attach each dropdown list to the cells it covers. Returns (lists used, cells covered).
async fn insert_dropdowns(conn: &mut sqlx::MySqlConnection, sch: &Schema, dropdowns: &[Dropdown], pos: &HashMap<usize, usize>, rn_of: &HashMap<usize, u64>) -> anyhow::Result<(usize, usize)> {
    let mut ids: HashMap<(String, i8), u64> = sqlx::query_as::<_, (u64, String, i8)>("SELECT id, CAST(options AS CHAR), strict FROM dropdown_lists").fetch_all(&mut *conn).await?.into_iter().map(|(i, o, s)| ((o, s), i)).collect();
    let last = rn_of.keys().max().copied().unwrap_or(0);
    let (mut used, mut cells): (HashSet<u64>, HashMap<(u64, String), u64>) = (HashSet::new(), HashMap::new());
    for d in dropdowns {
        let key = (serde_json::to_string(&d.options)?, d.strict as i8);
        let lid = match ids.get(&key) {
            Some(i) => *i,
            None => {
                let i = sqlx::query("INSERT INTO dropdown_lists (options, strict) VALUES (?, ?)").bind(&key.0).bind(key.1).execute(&mut *conn).await?.last_insert_id();
                ids.insert(key, i);
                i
            }
        };
        for (c1, c2, r1, r2) in &d.cells {
            for fc in *c1..=*c2 {
                let Some(&oc) = pos.get(&fc) else { continue };
                for n in *r1..=(*r2).min(last) {
                    if let Some(&rn) = rn_of.get(&n) {
                        cells.insert((rn, sch.cols[oc].field.clone()), lid);
                        used.insert(lid);
                    }
                }
            }
        }
    }
    let data: Vec<Vec<Val>> = cells.into_iter().map(|((rn, f), l)| vec![Val::I(rn as i64), Val::T(f), Val::I(l as i64)]).collect();
    let n = data.len();
    insert_conn(conn, "dropdown_cells", &["rn", "field", "list_id"], &data).await?;
    Ok((used.len(), n))
}

/// Give every order a dropdown in each added "Dropdown" column (new rows from an import included).
async fn attach_custom_dropdowns(conn: &mut sqlx::MySqlConnection, sch: &Schema) -> anyhow::Result<()> {
    for c in sch.cols.iter().filter(|c| c.custom && !c.deleted && c.dropdown) {
        let opts = serde_json::to_string(&c.options)?;
        let lid = match sqlx::query_scalar::<_, u64>("SELECT id FROM dropdown_lists WHERE CAST(options AS CHAR) = ? AND strict = 1 LIMIT 1").bind(&opts).fetch_optional(&mut *conn).await? {
            Some(i) => i,
            None => sqlx::query("INSERT INTO dropdown_lists (options, strict) VALUES (?, 1)").bind(&opts).execute(&mut *conn).await?.last_insert_id(),
        };
        sqlx::query("INSERT IGNORE INTO dropdown_cells (rn, field, list_id) SELECT rn, ?, ? FROM orders").bind(&c.field).bind(lid).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Only one import runs at a time (two at once would fight over the same tables and the small server's memory).
static IMPORT_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the page is told when a background import fails (the details are in the server log).
fn error_text(e: AppError) -> String {
    match e {
        AppError::Bad(t) | AppError::Denied(t) | AppError::Forbidden(t) | AppError::TooMany(t) => t,
        AppError::Unauthorized => "Please sign in.".into(),
        AppError::NotFound => "Not found".into(),
        AppError::Internal(err) => {
            tracing::error!("import failed: {err:#}");
            "Something went wrong on the server while importing. Nothing was changed.".into()
        }
    }
}

/// With `"background": true` the import keeps running on the server even if the connection closes (a proxy such as Cloudflare cuts
/// requests at 100 seconds, which a big sheet on a small server can exceed); the request answers at once and the page follows
/// `/api/progress?token=...`, which ends with `done: true` and the `result` or an `error`. Without it, the request waits for the result.
pub async fn run_import(_a: AdminUser, State(s): State<AppState>, Json(b): Json<Value>) -> AppResult<Json<Value>> {
    let mode = b.get("mode").and_then(Value::as_str).unwrap_or("").to_string();
    if !["replace", "append", "dropdowns"].contains(&mode.as_str()) {
        return Err(AppError::bad("Choose whether to replace or add to the existing orders."));
    }
    let (id, sheet) = (b.get("id").and_then(Value::as_str).unwrap_or("").to_string(), b.get("sheet").and_then(Value::as_str).unwrap_or("").to_string());
    let token: String = b.get("token").and_then(Value::as_str).unwrap_or("").chars().take(40).collect();
    if !s.uploads.lock().unwrap().contains_key(&id) {
        return Err(AppError::bad("That upload is no longer on the server. Choose the file again."));
    }
    if IMPORT_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err(AppError::bad("An import is already running. Wait for it to finish."));
    }
    let guard = RunGuard; // frees the "one import at a time" lock when this import ends, even if it panics
    if b.get("background").and_then(Value::as_bool) == Some(true) && !token.is_empty() {
        tokio::spawn(async move {
            let _guard = guard;
            let res = do_import(&s, &mode, &id, &sheet, &token).await;
            let done = match res {
                Ok(Json(result)) => json!({ "pct": 100, "text": "Done.", "done": true, "result": result }),
                Err(e) => json!({ "pct": null, "text": "Failed", "done": true, "error": error_text(e) }),
            };
            if let Ok(mut p) = s.progress.lock() {
                p.insert(token, done);
            }
        });
        return Ok(Json(json!({ "started": true })));
    }
    let _guard = guard;
    do_import(&s, &mode, &id, &sheet, &token).await
}

struct RunGuard;

impl Drop for RunGuard {
    fn drop(&mut self) {
        IMPORT_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn do_import(s: &AppState, mode: &str, id: &str, sheet: &str, token: &str) -> AppResult<Json<Value>> {
    let (mode, id, sheet, token) = (mode.to_string(), id.to_string(), sheet.to_string(), token.to_string());
    let s = s.clone();
    let Some((path, filename)) = s.uploads.lock().unwrap().get(&id).cloned() else { return Err(AppError::bad("That upload is no longer on the server. Choose the file again.")) };
    let sch = s.schema();
    // the bar: reading the file 0-55%, saving the orders 55-90%, the rest (dropdowns, calculated columns, swap) up to 100%
    set_progress(&s, &token, Some(0.0), "Reading the orders from the file…");
    let (s2, t2, sch2, p2, sheet2) = (s.clone(), token.clone(), sch.clone(), path.clone(), sheet.clone());
    let parsed = tokio::task::spawn_blocking(move || {
        let mut tick = |f: f64| set_progress(&s2, &t2, Some(f * 55.0), "Reading the orders from the file…");
        parse_orders(&p2, &sheet2, &sch2, Some(&mut tick))
    })
    .await??;
    if parsed.rows.is_empty() {
        return Err(AppError::bad("The tab has a header row but no orders under it."));
    }
    set_progress(&s, &token, Some(55.0), &format!("Read {} orders. Saving them…", parsed.rows.len()));
    let stamp = json!({ "file": filename, "sheet": sheet, "mode": mode, "at": clock::now().format("%Y-%m-%dT%H:%M:%S").to_string() });

    let mut tx = s.db.begin().await?;
    sqlx::query("SET FOREIGN_KEY_CHECKS = 0").execute(&mut *tx).await?;
    let (lists, cells);
    if mode == "dropdowns" {
        // keep the orders and edits as they are; only reload the dropdown lists. Rows are matched by sheet row number (which is how a
        // "replace" import numbers them), and checked against the Label Number so a wrong file is caught.
        let label_c = sch.c("Label Number");
        let labels: HashMap<u64, String> = sqlx::query(&format!("SELECT rn, {label_c} FROM orders_text")).fetch_all(&mut *tx).await?.iter().map(|r| (r.try_get::<u64, _>(0).unwrap_or(0), r.try_get_unchecked::<String, _>(1).unwrap_or_default())).collect();
        let rn_of: HashMap<usize, u64> = parsed.rows.iter().filter(|(n, v)| labels.get(&(*n as u64)).is_some_and(|l| *l == v[0])).map(|(n, _)| (*n, *n as u64)).collect();
        if (rn_of.len() as f64) < 0.9 * labels.len() as f64 {
            return Err(AppError::bad("This file does not line up with the orders in the app (the Label Numbers differ), so its dropdowns were not applied. Use it with 'Replace everything' instead."));
        }
        sqlx::query("DELETE FROM dropdown_cells").execute(&mut *tx).await?;
        sqlx::query("DELETE FROM dropdown_lists").execute(&mut *tx).await?;
        (lists, cells) = insert_dropdowns(&mut tx, &sch, &parsed.dropdowns, &parsed.pos, &rn_of).await?;
        attach_custom_dropdowns(&mut tx, &sch).await?;
    } else {
        let rn_of: HashMap<usize, u64> = if mode == "replace" {
            for t in ["order_custom_values", "dropdown_cells", "dropdown_lists", "order_edits", "edit_log", "scans", "ship_events", "boxes", "trash_rows", "orders"] {
                sqlx::query(&format!("DELETE FROM `{t}`")).execute(&mut *tx).await?;
            }
            parsed.rows.iter().map(|(n, _)| (*n, *n as u64)).collect()
        } else {
            let top: Option<u64> = sqlx::query_scalar("SELECT MAX(rn) FROM orders").fetch_one(&mut *tx).await?;
            let top_trash: Option<u64> = sqlx::query_scalar("SELECT MAX(rn) FROM trash_rows").fetch_one(&mut *tx).await?;
            let nxt = top.unwrap_or(2).max(top_trash.unwrap_or(2)) + 1;
            parsed.rows.iter().enumerate().map(|(i, (n, _))| (*n, nxt + i as u64)).collect()
        };
        let s3 = s.clone();
        let t3 = token.clone();
        let mut tick = move |f: f64| set_progress(&s3, &t3, Some(55.0 + f * 35.0), &format!("Saving the orders… {}%", (f * 100.0) as i64));
        insert_orders(&mut tx, &sch, &parsed.rows, &rn_of, &mut tick).await?;
        set_progress(&s, &token, Some(92.0), "Adding the dropdown lists and calculated columns…");
        (lists, cells) = insert_dropdowns(&mut tx, &sch, &parsed.dropdowns, &parsed.pos, &rn_of).await?;
        attach_custom_dropdowns(&mut tx, &sch).await?;
        recompute_formulas(&mut tx, &sch, None).await?;
        set_progress(&s, &token, Some(97.0), "Saving…");
        sqlx::query("REPLACE INTO app_settings (k, v) VALUES ('orders.imported', ?)").bind(stamp.to_string()).execute(&mut *tx).await?;
    }
    sqlx::query("SET FOREIGN_KEY_CHECKS = 1").execute(&mut *tx).await?;
    tx.commit().await?;
    let _ = std::fs::remove_file(&path);
    s.uploads.lock().unwrap().remove(&id);
    set_progress(&s, &token, Some(100.0), "Done.");
    s.bump();
    Ok(Json(json!({
        "rows": if mode == "dropdowns" { 0 } else { parsed.rows.len() }, "matched": parsed.matched, "ignored": parsed.ignored, "missing": parsed.missing, "skipped": parsed.skipped,
        "dropdowns_unsupported": parsed.dropdowns_unsupported, "dropdown_lists": lists, "dropdown_cells": cells,
    })))
}
