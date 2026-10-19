//! probe-server: a small rehearsal of the production backend.
//! CRUD on `records` over HTTPS (REST), WSS and MQTT, with every change logged
//! to a TimescaleDB hypertable and pushed live to all clients.

pub mod auth;
pub mod config;
mod error;
mod http;
mod mqtt;
mod records;
mod ws;

use std::{
    net::SocketAddr,
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

use auth::Auth;
pub use config::Config;
use records::Records;

/// Shared by all request handlers. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    pool: PgPool,
    records: Records,
    auth: Auth,
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

    let auth = Auth::new(pool.clone());
    let cleanup = tokio::spawn(delete_expired_sessions(auth.clone(), shutdown.clone()));

    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        records,
        auth,
        mqtt_connected,
        shutdown: shutdown.clone(),
    };

    tracing::info!(addr = %listener.local_addr()?, "listening");
    // With the TCP peer address, for login rate limiting without a proxy.
    let app = http::router(state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .context("http server failed")?;

    // HTTP is drained; let MQTT say goodbye, then close the pool.
    let _ = mqtt.await;
    let _ = cleanup.await;
    pool.close().await;
    tracing::info!("stopped");
    Ok(())
}

/// Connects to the database for the user commands (`create-user` etc.),
/// running pending migrations so the tables exist.
pub async fn connect_for_admin(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await
        .context("cannot connect to the database")?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("database migration failed")?;
    Ok(pool)
}

/// Once an hour, deletes sessions that have timed out.
async fn delete_expired_sessions(auth: Auth, shutdown: CancellationToken) {
    let mut tick = tokio::time::interval(Duration::from_secs(60 * 60));
    loop {
        tokio::select! {
            _ = tick.tick() => match auth.delete_expired().await {
                Ok(0) => {}
                Ok(n) => tracing::info!(sessions = n, "deleted expired sessions"),
                Err(e) => tracing::warn!(error = %e, "deleting expired sessions failed"),
            },
            () = shutdown.cancelled() => break,
        }
    }
}
