//! probe-server: a small rehearsal of the production backend.
//! CRUD on `records` over HTTPS (REST), WSS and MQTT, with every change logged
//! to a TimescaleDB hypertable and pushed live to all clients.

pub mod config;
mod error;
mod http;
mod mqtt;
mod records;
mod ws;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

pub use config::Config;
use records::Records;

/// Shared by all request handlers. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    pool: PgPool,
    records: Records,
    mqtt_connected: Arc<AtomicBool>,
    shutdown: CancellationToken,
}

impl AppState {
    fn mqtt_connected(&self) -> bool {
        self.mqtt_connected.load(Ordering::Relaxed)
    }
}

/// Connects to the database (running pending migrations) and the broker, then
/// serves HTTP on `listener` until `shutdown` is cancelled.
pub async fn run(
    config: Config,
    listener: TcpListener,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .context("cannot connect to the database")?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("database migration failed")?;
    tracing::info!("database ready");
    serve(config, pool, listener, shutdown).await
}

/// Like [`run`], but with an existing, already migrated pool (used by the
/// integration tests, which get a fresh database per test).
pub async fn serve(
    config: Config,
    pool: PgPool,
    listener: TcpListener,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let records = Records::new(pool.clone());
    let mqtt_connected = Arc::new(AtomicBool::new(false));
    let mqtt = mqtt::spawn(
        &config.mqtt,
        records.clone(),
        mqtt_connected.clone(),
        shutdown.clone(),
    );

    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        records,
        mqtt_connected,
        shutdown: shutdown.clone(),
    };

    tracing::info!(addr = %listener.local_addr()?, "listening");
    axum::serve(listener, http::router(state))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .context("http server failed")?;

    // HTTP is drained; let MQTT say goodbye, then close the pool.
    let _ = mqtt.await;
    pool.close().await;
    tracing::info!("stopped");
    Ok(())
}
