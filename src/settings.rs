//! Small settings kept as one JSON value each in `app_settings`: branding, the Dashboard's operators, locks, filter views ...

use serde_json::Value;
use sqlx::MySqlPool;

use crate::error::AppResult;

pub async fn get(db: &MySqlPool, key: &str) -> AppResult<Option<Value>> {
    let text: Option<String> = sqlx::query_scalar("SELECT CAST(v AS CHAR) FROM app_settings WHERE k = ?").bind(key).fetch_optional(db).await?;
    Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
}

pub async fn put(db: &MySqlPool, key: &str, value: &Value) -> AppResult<()> {
    sqlx::query("INSERT INTO app_settings (k, v) VALUES (?, ?) ON DUPLICATE KEY UPDATE v = VALUES(v)").bind(key).bind(value.to_string()).execute(db).await?;
    Ok(())
}
