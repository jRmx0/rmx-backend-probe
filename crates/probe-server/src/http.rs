//! HTTP routes. Everything lives under `/probe`, because Caddy forwards
//! `api.rmx.lt/probe/*` without stripping the prefix.

use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{any, get},
};
use probe_protocol::{ClientConfig, ConnectionType, Health, RecordInput, RecordRow};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use crate::{AppState, error::AppError, ws};

pub fn router(state: AppState) -> Router {
    let mut probe = Router::new()
        .route("/api/records", get(list).post(create))
        .route(
            "/api/records/{id}",
            axum::routing::put(update).delete(delete),
        )
        .route("/ws", get(ws::upgrade))
        .route("/health", get(health))
        .route("/config.json", get(client_config))
        // Unknown API paths are a plain 404, not the app's index.html.
        .route("/api/{*rest}", any(|| async { StatusCode::NOT_FOUND }));

    let mut app = Router::new().route("/probe", get(|| async { Redirect::permanent("/probe/") }));

    // The built client (production only). Unknown paths fall back to
    // index.html so client-side routes still load the app.
    if let Some(dir) = &state.config.static_dir {
        let index = ServeFile::new(dir.join("index.html"));
        // `nest` does not match "/probe/" itself, so route it explicitly.
        app = app.route_service("/probe/", index.clone());
        probe = probe.fallback_service(ServeDir::new(dir).fallback(index));
    }

    app.nest("/probe", probe)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<RecordRow>>, AppError> {
    Ok(Json(state.records.list().await?))
}

async fn create(
    State(state): State<AppState>,
    Json(input): Json<RecordInput>,
) -> Result<(StatusCode, Json<RecordRow>), AppError> {
    let record = state.records.create(ConnectionType::Https, input).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<RecordInput>,
) -> Result<Json<RecordRow>, AppError> {
    Ok(Json(
        state
            .records
            .update(ConnectionType::Https, id, input)
            .await?,
    ))
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    state.records.delete(ConnectionType::Https, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 200 when the database answers and the MQTT connection is up, else 503.
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let db = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar!("SELECT 1").fetch_one(&state.pool),
    )
    .await
    .is_ok_and(|r| r.is_ok());
    let mqtt = state.mqtt_connected();
    let ok = db && mqtt;
    let body = Health {
        status: if ok { "ok" } else { "degraded" }.into(),
        db,
        mqtt,
        version: env!("CARGO_PKG_VERSION").into(),
    };
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body))
}

async fn client_config(State(state): State<AppState>) -> Json<ClientConfig> {
    Json(ClientConfig {
        mqtt_ws_url: state.config.mqtt_ws_url.clone(),
        mqtt_topic_prefix: state.config.mqtt.topic_prefix.clone(),
    })
}
