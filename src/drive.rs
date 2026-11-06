//! Names of the Google Drive files behind the Shipping Label Link column.
//!
//! The label PDFs are named after the customer and the tracking number ("Lisa-Norris---9334611043900227102757.pdf"), so the name is how
//! the team checks that the attached link is the right one. A file shared by link shows its name in the title of its Drive page; the app
//! reads that title once, in the background, and keeps it in `drive_names`. With a Google API key (DRIVE_API_KEY, or the file
//! data/drive_key.txt) the Drive API answers instead: fast and with a generous limit. When Google answers "too many requests" nothing is
//! saved for that file: the read pauses (30 s, 1 min, 2 min ... up to 10) and tries the same file again. Nothing here sends any of the
//! app's data to Google: only the file id in the link is asked for.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use regex::Regex;
use serde_json::{json, Value};
use sqlx::MySqlPool;

use crate::clock;

/// Seconds before a file that could not be read is tried again.
pub const RETRY: i64 = 3600;

pub struct Limited;

#[derive(Default)]
struct Inner {
    queue: VecDeque<String>,
    queued: HashSet<String>,
    limited_until: Option<Instant>,
    step: u64,
}

pub struct Drive {
    inner: Mutex<Inner>,
    running: AtomicBool,
    cleaned: AtomicBool,
    api_key: String,
    client: reqwest::Client,
}

impl Drive {
    pub fn new(data_dir: &str) -> Arc<Self> {
        let mut key = std::env::var("DRIVE_API_KEY").unwrap_or_default().trim().to_string();
        if key.is_empty() {
            key = std::fs::read_to_string(format!("{data_dir}/drive_key.txt")).unwrap_or_default().trim().to_string();
        }
        let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().expect("http client");
        Arc::new(Self { inner: Mutex::default(), running: AtomicBool::new(false), cleaned: AtomicBool::new(false), api_key: key, client })
    }

    /// What the background read is doing: how many files are waiting, and whether it is paused because Google is limiting it.
    pub fn state(&self) -> Value {
        let i = self.inner.lock().unwrap();
        json!({ "queued": i.queue.len(), "limited": i.limited_until.is_some_and(|t| Instant::now() < t), "key": !self.api_key.is_empty() })
    }

    /// The file's name, "" when it cannot be read (private, deleted); Err(Limited) when Google is limiting the requests.
    async fn fetch_name(&self, fid: &str) -> Result<String, Limited> {
        if !self.api_key.is_empty() {
            let url = format!("https://www.googleapis.com/drive/v3/files/{fid}?fields=name&supportsAllDrives=true&key={}", self.api_key);
            let r = self.client.get(url).send().await.map_err(|_| Limited)?;
            let code = r.status().as_u16();
            if code == 429 || (500..600).contains(&code) {
                return Err(Limited);
            }
            let body = r.text().await.map_err(|_| Limited)?;
            if code == 403 && body.contains("ateLimit") {
                return Err(Limited);
            }
            if code != 200 {
                return Ok(String::new()); // 403 / 404: not shared, or gone
            }
            return Ok(serde_json::from_str::<Value>(&body).ok().and_then(|v| v["name"].as_str().map(str::to_string)).unwrap_or_default());
        }
        let r = self.client.get(format!("https://drive.google.com/file/d/{fid}/view")).header("User-Agent", "Mozilla/5.0").send().await.map_err(|_| Limited)?;
        let code = r.status().as_u16();
        if code == 429 || (500..600).contains(&code) {
            return Err(Limited);
        }
        if code != 200 {
            return Ok(String::new()); // 403 / 404: private or deleted
        }
        let page = r.text().await.map_err(|_| Limited)?;
        let page: String = page.chars().take(80_000).collect();
        let title = Regex::new(r"(?s)<title>(.*?)</title>").unwrap().captures(&page).map(|m| unescape(m[1].trim())).unwrap_or_default();
        let title = Regex::new(r"\s*-\s*Google Drive\s*$").unwrap().replace(&title, "").to_string();
        let low = title.to_lowercase();
        Ok(if title.is_empty() || low == "google drive" || low.contains("sign in") || low == "google drive: sign-in" { String::new() } else { title })
    }
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&#x27;", "'")
}

fn id_ok(s: &str) -> bool {
    s.len() >= 25 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// {id: (name, when it was read)} for every file the app has looked at.
pub async fn all_names(db: &MySqlPool) -> anyhow::Result<HashMap<String, (String, NaiveDateTime)>> {
    let rows: Vec<(String, String, NaiveDateTime)> = sqlx::query_as("SELECT CAST(id AS CHAR), CAST(name AS CHAR), read_at FROM drive_names").fetch_all(db).await?;
    Ok(rows.into_iter().map(|(i, n, t)| (i, (n, t))).collect())
}

async fn save(db: &MySqlPool, fid: &str, name: &str) {
    let _ = sqlx::query("INSERT INTO drive_names (id, name, read_at) VALUES (?,?,?) ON DUPLICATE KEY UPDATE name = VALUES(name), read_at = VALUES(read_at)").bind(fid).bind(name).bind(clock::now()).execute(db).await;
}

/// The page asks for the names of the files in view: the ones already known come back now, the rest are read in the background.
pub async fn names(drive: &Arc<Drive>, db: &MySqlPool, ids: Vec<String>) -> anyhow::Result<Value> {
    let mut seen = HashSet::new();
    let ids: Vec<String> = ids.into_iter().filter(|i| id_ok(i) && seen.insert(i.clone())).take(80).collect();
    let known = all_names(db).await?;
    let now = clock::now();
    let (mut got, mut pending) = (serde_json::Map::new(), 0);
    for id in ids {
        match known.get(&id) {
            Some((name, at)) if !name.is_empty() || (now - *at).num_seconds() < RETRY => {
                got.insert(id, json!(name));
            }
            _ => {
                pending += 1;
                let fresh = drive.inner.lock().unwrap().queued.insert(id.clone());
                if fresh {
                    let (d, db) = (drive.clone(), db.clone());
                    tokio::spawn(async move {
                        if let Ok(name) = d.fetch_name(&id).await {
                            save(&db, &id, &name).await;
                        }
                        d.inner.lock().unwrap().queued.remove(&id);
                    });
                }
            }
        }
    }
    Ok(json!({ "names": got, "pending": pending }))
}

/// Read many file names slowly in the background, one file at a time, in the order given (the lookups above stay quick).
pub fn crawl(drive: &Arc<Drive>, db: &MySqlPool, ids: Vec<String>) {
    {
        let mut i = drive.inner.lock().unwrap();
        for id in ids {
            if id_ok(&id) && i.queued.insert(id.clone()) {
                i.queue.push_back(id);
            }
        }
        if i.queue.is_empty() {
            return;
        }
    }
    if drive.running.swap(true, Ordering::SeqCst) {
        return;
    }
    let (d, db) = (drive.clone(), db.clone());
    tokio::spawn(async move {
        // once: files saved as "could not be read" earlier may only have been refused for asking too fast. Ask again.
        if !d.cleaned.swap(true, Ordering::SeqCst) {
            let _ = sqlx::query("DELETE FROM drive_names WHERE name = ''").execute(&db).await;
        }
        loop {
            let next = d.inner.lock().unwrap().queue.pop_front();
            let Some(fid) = next else {
                d.running.store(false, Ordering::SeqCst);
                return;
            };
            match d.fetch_name(&fid).await {
                Ok(name) => {
                    d.inner.lock().unwrap().step = 0;
                    save(&db, &fid, &name).await;
                    d.inner.lock().unwrap().queued.remove(&fid);
                    tokio::time::sleep(Duration::from_millis(if d.api_key.is_empty() { 1000 } else { 100 })).await;
                }
                Err(Limited) => {
                    let wait = {
                        let mut i = d.inner.lock().unwrap();
                        i.queue.push_front(fid); // try this one again after the pause
                        i.step = (i.step * 2).clamp(30, 600);
                        i.limited_until = Some(Instant::now() + Duration::from_secs(i.step));
                        i.step
                    };
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
            }
        }
    });
}
