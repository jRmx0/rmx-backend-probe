//! Wire types shared by `probe-server` and the browser client.
//!
//! Everything here is plain JSON over the wire. The same request and response
//! envelopes are used on WSS and MQTT, so a client can switch transports without
//! changing its message handling. TypeScript copies are generated with `ts-rs`
//! into `client/src/generated/` (run `cargo test -p probe-protocol`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Longest accepted `value`, in characters.
pub const MAX_VALUE_CHARS: usize = 1000;

/// The transport that created or last changed a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[cfg_attr(feature = "sqlx", derive(sqlx::Type))]
#[cfg_attr(
    feature = "sqlx",
    sqlx(type_name = "connection_type", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ConnectionType {
    Https,
    Wss,
    Mqtt,
}

impl ConnectionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Wss => "wss",
            Self::Mqtt => "mqtt",
        }
    }
}

/// The kind of change logged in `record_events`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[cfg_attr(feature = "sqlx", derive(sqlx::Type))]
#[cfg_attr(
    feature = "sqlx",
    sqlx(type_name = "record_op", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum RecordOp {
    Create,
    Update,
    Delete,
}

/// One row of the `records` table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RecordRow {
    #[ts(type = "number")]
    pub id: i64,
    /// When the record was created.
    pub time: DateTime<Utc>,
    pub connection_type: ConnectionType,
    pub value: Option<String>,
    pub updated_at: DateTime<Utc>,
    /// The client IP that created the record (an update keeps it). `None`
    /// for records created over MQTT: the broker doesn't pass on the
    /// client's address, so the server can't know it.
    pub created_ip: Option<String>,
}

/// Body of `POST /probe/api/records` and `PUT /probe/api/records/{id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RecordInput {
    pub value: Option<String>,
}

impl RecordInput {
    /// Checks the input. Returns a message suitable for the client on failure.
    pub fn validate(&self) -> Result<(), String> {
        match &self.value {
            Some(v) if v.chars().count() > MAX_VALUE_CHARS => {
                Err(format!("value is longer than {MAX_VALUE_CHARS} characters"))
            }
            _ => Ok(()),
        }
    }
}

/// A CRUD operation, sent by the client over WSS or MQTT.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "lowercase")]
#[ts(export)]
pub enum Request {
    List,
    Create {
        value: Option<String>,
    },
    Update {
        #[ts(type = "number")]
        id: i64,
        value: Option<String>,
    },
    Delete {
        #[ts(type = "number")]
        id: i64,
    },
}

/// A request plus the id the client uses to match the response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RequestEnvelope {
    pub request_id: String,
    pub request: Request,
}

/// The answer to one [`RequestEnvelope`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResponseEnvelope {
    /// Copied from the request. Empty if the request could not be parsed at all.
    pub request_id: String,
    pub result: RpcResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "lowercase")]
#[ts(export)]
pub enum RpcResult {
    Ok { data: RpcData },
    Error { error: ApiError },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[ts(export)]
pub enum RpcData {
    Records {
        records: Vec<RecordRow>,
    },
    Record {
        record: RecordRow,
    },
    Deleted {
        #[ts(type = "number")]
        id: i64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    Internal,
    /// No valid session (REST: 401), or wrong username or password at login.
    Unauthorized,
    /// The request came from a page on another origin (REST: 403).
    Forbidden,
    /// Too many failed logins; wait `Retry-After` seconds (REST: 429).
    TooManyRequests,
}

/// Error body, used by REST (with a matching HTTP status) and in [`RpcResult::Error`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

/// A change to `records`, broadcast to every client whatever transport caused it.
/// Sent on WSS (inside [`ServerMessage::Event`]) and on the MQTT events topic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "lowercase")]
#[ts(export)]
pub enum ChangeEvent {
    Create {
        record: RecordRow,
    },
    Update {
        record: RecordRow,
    },
    Delete {
        #[ts(type = "number")]
        id: i64,
        connection_type: ConnectionType,
    },
}

impl ChangeEvent {
    pub fn record_id(&self) -> i64 {
        match self {
            Self::Create { record } | Self::Update { record } => record.id,
            Self::Delete { id, .. } => *id,
        }
    }
}

/// Every message the server sends on the WebSocket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "lowercase")]
#[ts(export)]
pub enum ServerMessage {
    Response { response: ResponseEnvelope },
    Event { event: ChangeEvent },
}

/// `GET /probe/health`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Health {
    /// `"ok"` when the database and the MQTT broker are both reachable, else `"degraded"`.
    pub status: String,
    pub db: bool,
    pub mqtt: bool,
    pub version: String,
}

/// Body of `POST /probe/api/auth/login`.
#[derive(Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

// Hand-written so the password never ends up in a log line.
impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// The logged-in user: the answer to a login and to `GET /probe/api/auth/me`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Me {
    pub username: String,
}

/// MQTT topic layout. A client puts its own id in the topic, like mowers will
/// (`mower/<id>/...`), so a broker ACL can scope each login later.
///
/// - `<prefix>/req/<client-id>`: client → server, payload [`RequestEnvelope`]
/// - `<prefix>/res/<client-id>`: server → client, payload [`ResponseEnvelope`]
/// - `<prefix>/events/records`: server → all, payload [`ChangeEvent`]
/// - `<prefix>/status/server`: retained `online` / `offline` (last will)
pub mod topics {
    pub fn request(prefix: &str, client_id: &str) -> String {
        format!("{prefix}/req/{client_id}")
    }

    /// Subscription filter that matches every client's requests.
    pub fn request_filter(prefix: &str) -> String {
        format!("{prefix}/req/+")
    }

    pub fn response(prefix: &str, client_id: &str) -> String {
        format!("{prefix}/res/{client_id}")
    }

    pub fn events(prefix: &str) -> String {
        format!("{prefix}/events/records")
    }

    pub fn server_status(prefix: &str) -> String {
        format!("{prefix}/status/server")
    }

    /// Extracts the client id from a request topic. Returns `None` for any other
    /// topic, or for an id that is not a single plain topic level.
    pub fn parse_request<'a>(prefix: &str, topic: &'a str) -> Option<&'a str> {
        let id = topic.strip_prefix(prefix)?.strip_prefix("/req/")?;
        is_valid_client_id(id).then_some(id)
    }

    /// 1–64 characters of `[A-Za-z0-9_-]`, so the id is safe inside a topic.
    pub fn is_valid_client_id(id: &str) -> bool {
        (1..=64).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> RecordRow {
        let t = DateTime::parse_from_rfc3339("2026-10-18T12:00:00Z")
            .unwrap()
            .to_utc();
        RecordRow {
            id: 7,
            time: t,
            connection_type: ConnectionType::Mqtt,
            value: Some("hi".into()),
            updated_at: t,
            created_ip: Some("203.0.113.7".into()),
        }
    }

    #[test]
    fn request_json_shape() {
        let env: RequestEnvelope = serde_json::from_value(json!({
            "request_id": "r1",
            "request": { "op": "update", "id": 3, "value": null }
        }))
        .unwrap();
        assert_eq!(env.request, Request::Update { id: 3, value: None });

        let list: Request = serde_json::from_value(json!({ "op": "list" })).unwrap();
        assert_eq!(list, Request::List);
    }

    #[test]
    fn server_message_json_shape() {
        let msg = ServerMessage::Event {
            event: ChangeEvent::Delete {
                id: 7,
                connection_type: ConnectionType::Https,
            },
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({ "type": "event", "event": { "op": "delete", "id": 7, "connection_type": "https" } })
        );

        let msg = ServerMessage::Response {
            response: ResponseEnvelope {
                request_id: "r1".into(),
                result: RpcResult::Ok {
                    data: RpcData::Record { record: record() },
                },
            },
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["response"]["result"]["status"], "ok");
        assert_eq!(v["response"]["result"]["data"]["kind"], "record");
        assert_eq!(
            v["response"]["result"]["data"]["record"]["time"],
            "2026-10-18T12:00:00Z"
        );
        let back: ServerMessage = serde_json::from_value(v).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn validate_value_length() {
        assert!(RecordInput { value: None }.validate().is_ok());
        let ok = "ž".repeat(MAX_VALUE_CHARS);
        assert!(RecordInput { value: Some(ok) }.validate().is_ok());
        let long = "a".repeat(MAX_VALUE_CHARS + 1);
        assert!(RecordInput { value: Some(long) }.validate().is_err());
    }

    #[test]
    fn login_request_debug_hides_password() {
        let req = LoginRequest {
            username: "probe".into(),
            password: "hunter2".into(),
        };
        let debug = format!("{req:?}");
        assert!(
            debug.contains("probe") && !debug.contains("hunter2"),
            "{debug}"
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::TooManyRequests).unwrap(),
            "too_many_requests"
        );
    }

    #[test]
    fn request_topic_parsing() {
        assert_eq!(
            topics::parse_request("probe", "probe/req/web-1a"),
            Some("web-1a")
        );
        assert_eq!(topics::parse_request("probe", "probe/req/a/b"), None);
        assert_eq!(topics::parse_request("probe", "probe/req/"), None);
        assert_eq!(topics::parse_request("probe", "probe/res/x"), None);
        assert_eq!(topics::parse_request("probe", "probex/req/x"), None);
        assert_eq!(topics::response("probe", "x"), "probe/res/x");
        assert!(!topics::is_valid_client_id("a+b"));
        assert!(!topics::is_valid_client_id(&"a".repeat(65)));
    }
}
