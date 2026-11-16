//! Short-lived copies of read-only answers, kept in memory, so that many people asking the same question in the same moment make the
//! database count once. Nothing here is written to the database, and a copy is thrown away after a few seconds (or, for the All Orders
//! totals, the moment any order changes), so a number is never older than that.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long the Dashboard's numbers are shared between viewers.
pub const DASHBOARD_TTL: Duration = Duration::from_secs(5);
/// The longest an All Orders total is kept even when nothing changed (the row colours depend on the clock).
const TOTAL_TTL: Duration = Duration::from_secs(30);
/// A cache never grows beyond this many answers: it is emptied instead.
const MAX_ENTRIES: usize = 200;

#[derive(Default)]
pub struct Caches {
    /// Dashboard answers by the date asked for. Held across the counting, so viewers that arrive meanwhile wait for the one in progress.
    pub dashboard: tokio::sync::Mutex<HashMap<String, (Instant, Value)>>,
    /// "N orders" of All Orders by filter: (when, data version, total).
    totals: Mutex<HashMap<String, (Instant, u64, i64)>>,
}

impl Caches {
    pub async fn clear_dashboard(&self) {
        self.dashboard.lock().await.clear();
    }

    pub fn total_get(&self, key: &str, version: u64) -> Option<i64> {
        let m = self.totals.lock().ok()?;
        m.get(key).filter(|(at, v, _)| *v == version && at.elapsed() < TOTAL_TTL).map(|(_, _, n)| *n)
    }

    pub fn total_put(&self, key: String, version: u64, n: i64) {
        if let Ok(mut m) = self.totals.lock() {
            if m.len() >= MAX_ENTRIES {
                m.clear();
            }
            m.insert(key, (Instant::now(), version, n));
        }
    }
}

pub fn max_entries() -> usize {
    MAX_ENTRIES
}
