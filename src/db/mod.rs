pub mod history;
pub mod prioritized;

use sqlx::{migrate::Migrator, sqlite::SqliteConnectOptions, SqlitePool};
use std::{fs, sync::OnceLock};
use tracing::{info, warn};

pub(crate) static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

const DB_FILE: &str = "jd_history.db";

static POOL: OnceLock<SqlitePool> = OnceLock::new();

/// Shared database handle
pub fn pool() -> Option<&'static SqlitePool> {
    POOL.get()
}

pub async fn init() -> Result<(), sqlx::Error> {
    if POOL.get().is_some() {
        return Ok(());
    }
    let pool = match open_and_migrate().await {
        Ok(pool) => pool,
        Err(error) => {
            warn!(%error, "the history database could not be migrated; rebuilding it");
            for file in [
                DB_FILE.to_string(),
                format!("{DB_FILE}-wal"),
                format!("{DB_FILE}-shm"),
            ] {
                if let Err(error) = fs::remove_file(&file) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        warn!(%error, file, "could not remove part of the old history database");
                    }
                }
            }
            open_and_migrate().await?
        }
    };
    let _ = POOL.set(pool);
    info!("history database ready");
    prioritized::restore().await;
    Ok(())
}

async fn open_and_migrate() -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(DB_FILE)
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(options).await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}
