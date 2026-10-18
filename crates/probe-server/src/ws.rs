//! WebSocket transport at `/probe/ws`.
//!
//! Client → server: `RequestEnvelope` JSON text frames.
//! Server → client: `ServerMessage` JSON, either the response to a request or
//! a change event pushed whenever any transport changes a record.

use std::time::Duration;

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, Utf8Bytes, WebSocket, close_code},
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use probe_protocol::{
    ApiError, ConnectionType, ErrorCode, RequestEnvelope, ResponseEnvelope, RpcResult,
    ServerMessage,
};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

/// Keeps idle connections alive through proxies and detects dead peers.
const PING_INTERVAL: Duration = Duration::from_secs(30);

pub async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| handle(socket, state))
}

async fn handle(socket: WebSocket, state: AppState) {
    let (mut tx, mut rx) = socket.split();
    let mut events = state.records.subscribe();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.reset();
    tracing::debug!("websocket connected");

    loop {
        let outgoing = tokio::select! {
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let response = respond(&state, &text).await;
                    ServerMessage::Response { response }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => continue, // binary, ping and pong frames
                Some(Err(e)) => {
                    tracing::debug!(error = %e, "websocket read error");
                    break;
                }
            },
            event = events.recv() => match event {
                Ok(event) => ServerMessage::Event { event },
                Err(RecvError::Lagged(n)) => {
                    // The client misses some events. Closing makes it reconnect and list again.
                    tracing::warn!(missed = n, "websocket client lagged, closing");
                    let _ = tx.send(close(close_code::AGAIN, "lagged, reconnect")).await;
                    break;
                }
                Err(RecvError::Closed) => break,
            },
            _ = ping.tick() => {
                if tx.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
                continue;
            }
            () = state.shutdown.cancelled() => {
                let _ = tx.send(close(close_code::AWAY, "server shutting down")).await;
                break;
            }
        };

        let json = serde_json::to_string(&outgoing).expect("ServerMessage serializes");
        if tx.send(Message::Text(json.into())).await.is_err() {
            break;
        }
    }
    tracing::debug!("websocket disconnected");
}

async fn respond(state: &AppState, text: &str) -> ResponseEnvelope {
    match parse_request(text.as_bytes()) {
        Ok(env) => ResponseEnvelope {
            request_id: env.request_id,
            result: state.records.handle(ConnectionType::Wss, env.request).await,
        },
        Err(response) => response,
    }
}

/// Parses a `RequestEnvelope`. On failure returns the error response to send,
/// keeping the `request_id` if the message had one. Shared with MQTT.
pub fn parse_request(payload: &[u8]) -> Result<RequestEnvelope, ResponseEnvelope> {
    serde_json::from_slice(payload).map_err(|e| {
        let request_id = serde_json::from_slice::<serde_json::Value>(payload)
            .ok()
            .and_then(|v| v.get("request_id")?.as_str().map(str::to_owned))
            .unwrap_or_default();
        ResponseEnvelope {
            request_id,
            result: RpcResult::Error {
                error: ApiError {
                    code: ErrorCode::BadRequest,
                    message: format!("invalid request: {e}"),
                },
            },
        }
    })
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: Utf8Bytes::from_static(reason),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_request_keeps_request_id() {
        let err = parse_request(br#"{"request_id":"r9","request":{"op":"nope"}}"#).unwrap_err();
        assert_eq!(err.request_id, "r9");
        assert!(
            matches!(err.result, RpcResult::Error { error } if error.code == ErrorCode::BadRequest)
        );

        let err = parse_request(b"not json").unwrap_err();
        assert_eq!(err.request_id, "");

        let ok = parse_request(br#"{"request_id":"r1","request":{"op":"list"}}"#).unwrap();
        assert_eq!(ok.request_id, "r1");
    }
}
