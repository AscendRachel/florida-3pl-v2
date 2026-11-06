//! Settings, read from the environment (a `.env` file next to the program is read too).

use anyhow::Context;

pub struct Config {
    pub database_url: String,
    pub host: String,
    pub port: u16,
    /// The folder of the web pages (index.html, login.html, static/).
    pub web_dir: String,
    /// A folder for files kept next to the database: the Google API key (drive_key.txt), uploads for the import.
    pub data_dir: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .context("DATABASE_URL is not set. Copy .env.example to .env (it holds the MariaDB address and password).")?;
        Ok(Self {
            database_url,
            host: std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            port: std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8765),
            web_dir: std::env::var("WEB_DIR").unwrap_or_else(|_| "web".into()),
            data_dir: std::env::var("DATA_DIR").unwrap_or_else(|_| "data".into()),
        })
    }

    /// The database address without the password, for the log.
    pub fn database_host(&self) -> String {
        match self.database_url.rsplit_once('@') {
            Some((_, host)) => host.to_string(),
            None => "(database)".into(),
        }
    }
}
