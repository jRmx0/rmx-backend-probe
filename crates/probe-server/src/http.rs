//! HTTP routes. Everything lives under `/probe`, because Caddy forwards
//! `api.rmx.lt/probe/*` without stripping the prefix.

use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Redirect},
    routing::{any, get, post, put},
};
use probe_protocol::{ConnectionType, Health, RecordInput, RecordRow};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use crate::{AppState, auth, client_ip::ClientIp, error::AppError, records::Source, ws};

pub fn router(state: AppState) -> Router {
    // Fail closed: data and live connections are in `protected`, and every
    // route there needs a valid session. Only the routes in `public` work
    // without one: health (status only, for the deploy script and
    // monitoring), login/logout, and the client's static files (the same code
    // as in the public repository, no data).
    let protected = Router::new()
        .route("/api/records", get(list).post(create))
        .route("/api/records/{id}", put(update).delete(delete))
        .route("/api/auth/me", get(auth::routes::me))
        .route(
            "/api/auth/logout-all",
            post(auth::routes::logout_everywhere),
        )
        // The handshake is a normal GET with the cookie, checked here before
        // the upgrade. That's what fixes iPhones: WebKit sends cookies on
        // WebSocket handshakes (but not basic-auth credentials).
        .route("/ws", get(ws::upgrade))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::routes::require_session,
        ));

    let mut probe = Router::new()
        .route("/health", get(health))
        .route("/api/auth/login", post(auth::routes::login))
        .route("/api/auth/logout", post(auth::routes::logout))
        // Unknown API paths are a plain 404, not the app's index.html.
        .route("/api/{*rest}", any(|| async { StatusCode::NOT_FOUND }))
        .merge(protected);

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
        // CSRF: state-changing requests and WebSocket handshakes only from our own pages.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::routes::check_origin,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<RecordRow>>, AppError> {
    Ok(Json(state.records.list().await?))
}

async fn create(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(input): Json<RecordInput>,
) -> Result<(StatusCode, Json<RecordRow>), AppError> {
    let record = state.records.create(https(ip), input).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

async fn update(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(id): Path<i64>,
    Json(input): Json<RecordInput>,
) -> Result<Json<RecordRow>, AppError> {
    Ok(Json(state.records.update(https(ip), id, input).await?))
}

async fn delete(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    state.records.delete(https(ip), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn https(ip: std::net::IpAddr) -> Source {
    Source::new(ConnectionType::Https, Some(ip))
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
