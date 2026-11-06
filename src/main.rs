//! Florida 3PL, the Rust version.
//!
//!   florida-3pl                          start the server
//!   florida-3pl create-admin             make the first admin (asks for the username, name and password)
//!   florida-3pl import <old data folder> [--replace]
//!                                        copy the Python app's data (its `data` folder) into MariaDB

mod admin_api;
mod amazon;
mod auth;
mod cformat;
mod clock;
mod config;
mod dashboard;
mod drive;
mod error;
mod filters;
mod import;
mod notify;
mod orders_api;
mod orders_write;
mod returns;
mod routes;
mod scan;
mod schema;
mod security;
mod sheet_import;
mod settings;
mod team_api;
mod xlsx;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::MySqlPool;
use tracing_subscriber::EnvFilter;

/// What every request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub db: MySqlPool,
    pub config: Arc<config::Config>,
    /// Counts up whenever data changes; the pages poll it to refresh themselves (the old app's /api/version).
    pub version: Arc<std::sync::atomic::AtomicU64>,
    /// Recent failed sign-ins per address, so guessing passwords gets stopped.
    pub failed: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    /// The columns of All Orders (reloaded when an admin adds, renames or deletes one).
    pub schema: Arc<std::sync::RwLock<Arc<schema::Schema>>>,
    /// The Google Drive file names (label links), read in the background.
    pub drive: Arc<drive::Drive>,
    /// Amazon import previews waiting to be applied (kept an hour).
    pub amazon_plans: amazon::Plans,
    /// Files uploaded to the Import tab, waiting to be imported.
    pub uploads: sheet_import::Uploads,
    /// What a long import step is doing right now, by the token the page made.
    pub progress: sheet_import::Progress,
}

impl AppState {
    /// Something changed: open pages that watch /api/version refresh themselves.
    pub fn bump(&self) {
        self.version.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// The columns changed (added, renamed, deleted, restored): load them again and rebuild the text view.
    pub async fn reload_schema(&self) -> anyhow::Result<()> {
        let sch = schema::Schema::load(&self.db).await?;
        sch.make_view(&self.db).await?;
        *self.schema.write().unwrap() = Arc::new(sch);
        Ok(())
    }

    pub fn version_now(&self) -> u64 {
        self.version.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn schema(&self) -> Arc<schema::Schema> {
        self.schema.read().unwrap().clone()
    }
}

async fn connect(config: &config::Config) -> anyhow::Result<MySqlPool> {
    tracing::info!("connecting to the database at {}", config.database_host());
    let db = MySqlPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&config.database_url)
        .await
        .context("could not connect to MariaDB. Is it running? (docker compose up -d)")?;
    // builds the tables on a new database, and brings an older one up to date; each migration runs once
    sqlx::migrate!("./migrations").run(&db).await.context("the database migrations failed")?;
    Ok(db)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = config::Config::from_env()?;
    let db = connect(&config).await?;

    match args.first().map(String::as_str) {
        Some("create-admin") => return create_admin(&db).await,
        Some("import") => {
            let dir = args.get(1).context("usage: florida-3pl import <the old app's data folder> [--replace]")?;
            return import::run(&db, std::path::Path::new(dir), args.iter().any(|a| a == "--replace")).await;
        }
        Some(other) => anyhow::bail!("unknown command `{other}` (use create-admin or import)"),
        None => {}
    }

    let sch = schema::Schema::load(&db).await?;
    sch.make_view(&db).await.context("could not create the orders_text view")?;
    tracing::info!("{} columns, text view ready", sch.n());

    let data_dir = config.data_dir.clone();
    let addr = format!("{}:{}", config.host, config.port);
    let state = AppState {
        db,
        config: Arc::new(config),
        version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        failed: Arc::default(),
        schema: Arc::new(std::sync::RwLock::new(Arc::new(sch))),
        drive: drive::Drive::new(&data_dir),
        amazon_plans: Arc::default(),
        uploads: Arc::default(),
        progress: Arc::default(),
    };
    // files uploaded but never imported are removed at start
    if let Ok(rd) = std::fs::read_dir(std::path::Path::new(&state.config.data_dir).join("uploads")) {
        for f in rd.flatten() {
            let _ = std::fs::remove_file(f.path());
        }
    }
    returns::start_scheduler(state.clone());
    let app = routes::router(state).into_make_service_with_connect_info::<SocketAddr>();
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| format!("port {addr} is already in use"))?;
    tracing::info!("Florida 3PL is running at http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// The first admin is made here, on the server, not through the web page.
async fn create_admin(db: &MySqlPool) -> anyhow::Result<()> {
    use std::io::Write;
    let ask = |label: &str| -> anyhow::Result<String> {
        print!("{label}: ");
        std::io::stdout().flush()?;
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        Ok(s.trim().to_string())
    };
    let username = ask("Username")?;
    let name = ask("Display name")?;
    print!("Password (10+ characters): ");
    std::io::stdout().flush()?;
    let password = rpassword::read_password()?;
    match auth::add_user(db, &username, &name, &password, "admin", None).await {
        Ok(u) => println!("Admin account \"{u}\" created. Sign in at the app's address."),
        Err(error::AppError::Bad(t)) => anyhow::bail!(t),
        Err(_) => anyhow::bail!("could not create the account"),
    }
    Ok(())
}
