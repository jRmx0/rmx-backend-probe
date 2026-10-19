//! Integration tests against a real TimescaleDB and Mosquitto.
//!
//! Needs the dev compose stack (`docker compose -f compose.dev.yaml up -d --wait`)
//! and the settings from `.env`, then:
//!
//! ```text
//! cargo test -p probe-server --features integration-tests
//! ```
//!
//! `#[sqlx::test]` gives every test its own fresh, migrated database, and every
//! test uses its own MQTT topic prefix, so tests can run in parallel.
//! `TestApp::start` also creates a user and logs in, so `app.http` and
//! `app.ws()` are a logged-in browser on an allowed origin; `app.anon` is
//! a client without cookie or Origin.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use probe_protocol::{
    ChangeEvent, ConnectionType, Health, RecordRow, Request, RequestEnvelope, ResponseEnvelope,
    RpcData, RpcResult, ServerMessage, topics,
};
use probe_server::Config;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::json;
use sqlx::PgPool;
use tokio::{net::TcpStream, sync::mpsc, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{self, Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;

const WAIT: Duration = Duration::from_secs(5);
/// The page origin the tests pretend to be (in ALLOWED_ORIGINS).
const ORIGIN: &str = "http://probe.test";
const USER: &str = "tester";
const PASSWORD: &str = "correct horse battery staple";
const COOKIE: &str = "__Secure-probe_session";

/// One running probe-server on a random local port.
struct TestApp {
    base: String,
    prefix: String,
    pool: PgPool,
    /// Logged in as USER, sends Origin: ORIGIN.
    http: reqwest::Client,
    /// The `name=value` session cookie of `http`.
    cookie: String,
    /// No cookie, no Origin.
    anon: reqwest::Client,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl TestApp {
    async fn start(pool: PgPool) -> Self {
        Self::start_with(pool, None).await
    }

    async fn start_with(pool: PgPool, static_dir: Option<String>) -> Self {
        let _ = dotenvy::dotenv();
        let prefix = format!("probe/it-{:016x}", rand::random::<u64>());
        let config = {
            let prefix = prefix.clone();
            Config::from_lookup(move |key| match key {
                "MQTT_TOPIC_PREFIX" => Some(prefix.clone()),
                "MQTT_CLIENT_ID" => Some(format!("server-{}", prefix.replace('/', "-"))),
                "STATIC_DIR" => static_dir.clone(),
                "ALLOWED_ORIGINS" => Some(format!("{ORIGIN},http://other.allowed.test")),
                _ => std::env::var(key).ok(),
            })
            .expect("test config (is .env present?)")
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(probe_server::serve(
            config,
            pool.clone(),
            listener,
            shutdown.clone(),
        ));

        let mut app = Self {
            base,
            prefix,
            pool,
            http: reqwest::Client::new(),
            cookie: String::new(),
            anon: reqwest::Client::new(),
            shutdown,
            task,
        };
        app.wait_healthy().await;
        probe_server::auth::create_user(&app.pool, USER, PASSWORD)
            .await
            .unwrap();
        let res = app.login(USER, PASSWORD).await;
        assert_eq!(res.status(), 200);
        app.cookie = session_cookie(&res);
        app.http = client(Some(&app.cookie), Some(ORIGIN));
        app
    }

    /// `POST /probe/api/auth/login` from an allowed origin, without a cookie.
    async fn login(&self, username: &str, password: &str) -> reqwest::Response {
        self.anon
            .post(self.url("/api/auth/login"))
            .header("origin", ORIGIN)
            .json(&json!({ "username": username, "password": password }))
            .send()
            .await
            .unwrap()
    }

    /// Opens the WebSocket with the given cookie and Origin headers.
    async fn ws_with(
        &self,
        cookie: Option<&str>,
        origin: Option<&str>,
    ) -> Result<Ws, tungstenite::Error> {
        let url = format!("{}/ws", self.url("").replacen("http", "ws", 1));
        let mut req = url.into_client_request().unwrap();
        if let Some(c) = cookie {
            req.headers_mut().insert("cookie", c.parse().unwrap());
        }
        if let Some(o) = origin {
            req.headers_mut().insert("origin", o.parse().unwrap());
        }
        let (stream, _) = tokio_tungstenite::connect_async(req).await?;
        Ok(Ws(stream))
    }

    fn url(&self, path: &str) -> String {
        format!("{}/probe{path}", self.base)
    }

    /// Waits until the server reports both the DB and MQTT as up.
    async fn wait_healthy(&self) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            if let Ok(res) = self.http.get(self.url("/health")).send().await
                && res.status() == 200
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "server not healthy in time"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// A logged-in WebSocket from an allowed origin.
    async fn ws(&self) -> Ws {
        self.ws_with(Some(&self.cookie), Some(ORIGIN))
            .await
            .unwrap()
    }

    async fn stop(self) {
        self.shutdown.cancel();
        timeout(WAIT, self.task)
            .await
            .expect("graceful shutdown in time")
            .unwrap()
            .unwrap();
    }
}

/// A reqwest client that sends these headers on every request.
fn client(cookie: Option<&str>, origin: Option<&str>) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(c) = cookie {
        headers.insert("cookie", c.parse().unwrap());
    }
    if let Some(o) = origin {
        headers.insert("origin", o.parse().unwrap());
    }
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}

/// The `name=value` part of the session `Set-Cookie` header.
fn session_cookie(res: &reqwest::Response) -> String {
    let set = set_cookie(res).expect("a Set-Cookie header");
    let pair = set.split(';').next().unwrap().to_string();
    assert!(pair.starts_with(&format!("{COOKIE}=")), "{set}");
    pair
}

fn set_cookie(res: &reqwest::Response) -> Option<String> {
    res.headers()
        .get("set-cookie")
        .map(|v| v.to_str().unwrap().to_string())
}

/// The HTTP status a refused WebSocket handshake got.
fn ws_refused(result: Result<Ws, tungstenite::Error>) -> u16 {
    match result {
        Err(tungstenite::Error::Http(res)) => res.status().as_u16(),
        Err(e) => panic!("expected an HTTP refusal, got {e}"),
        Ok(_) => panic!("the WebSocket was accepted"),
    }
}

struct Ws(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Ws {
    async fn send(&mut self, request_id: &str, request: Request) {
        let env = RequestEnvelope {
            request_id: request_id.into(),
            request,
        };
        let text = serde_json::to_string(&env).unwrap();
        self.0.send(Message::Text(text.into())).await.unwrap();
    }

    async fn next(&mut self) -> ServerMessage {
        loop {
            let msg = timeout(WAIT, self.0.next())
                .await
                .expect("websocket message in time")
                .unwrap()
                .unwrap();
            if let Message::Text(text) = msg {
                return serde_json::from_str(&text).unwrap();
            }
        }
    }

    /// The response to `request_id`, skipping events.
    async fn response(&mut self, request_id: &str) -> RpcResult {
        loop {
            if let ServerMessage::Response { response } = self.next().await {
                assert_eq!(response.request_id, request_id);
                return response.result;
            }
        }
    }

    /// The next change event, skipping responses.
    async fn event(&mut self) -> ChangeEvent {
        loop {
            if let ServerMessage::Event { event } = self.next().await {
                return event;
            }
        }
    }
}

/// A device-like MQTT client (like the LTE modem probe) logged in as probe-device.
struct Mqtt {
    client: AsyncClient,
    id: String,
    prefix: String,
    rx: mpsc::UnboundedReceiver<(String, Vec<u8>)>,
    /// Messages received while waiting for another topic.
    buffered: Vec<(String, Vec<u8>)>,
}

impl Mqtt {
    async fn connect(prefix: &str) -> Self {
        let id = format!("it-{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff);
        let host = std::env::var("MQTT_HOST").unwrap();
        let port = std::env::var("MQTT_PORT").map_or(1883, |p| p.parse().unwrap());
        let password = std::env::var("PROBE_DEVICE_PASSWORD").expect("PROBE_DEVICE_PASSWORD");
        let mut options = MqttOptions::new(&id, host, port);
        options.set_credentials("probe-device", password);
        options.set_max_packet_size(1024 * 1024, 64 * 1024);
        let (client, mut eventloop) = AsyncClient::new(options, 16);

        let (tx, rx) = mpsc::unbounded_channel();
        let (subscribed_tx, mut subscribed) = mpsc::channel(1);
        tokio::spawn(async move {
            while let Ok(event) = eventloop.poll().await {
                match event {
                    Event::Incoming(Packet::Publish(p)) => {
                        let _ = tx.send((p.topic, p.payload.to_vec()));
                    }
                    Event::Incoming(Packet::SubAck(_)) => {
                        let _ = subscribed_tx.try_send(());
                    }
                    _ => {}
                }
            }
        });

        for topic in [
            topics::response(prefix, &id),
            topics::events(prefix),
            topics::server_status(prefix),
        ] {
            client.subscribe(topic, QoS::AtLeastOnce).await.unwrap();
            timeout(WAIT, subscribed.recv())
                .await
                .expect("suback")
                .unwrap();
        }
        Self {
            client,
            id,
            prefix: prefix.into(),
            rx,
            buffered: Vec::new(),
        }
    }

    async fn send(&self, request_id: &str, request: Request) {
        let env = RequestEnvelope {
            request_id: request_id.into(),
            request,
        };
        self.send_raw(serde_json::to_vec(&env).unwrap()).await;
    }

    async fn send_raw(&self, payload: Vec<u8>) {
        self.client
            .publish(
                topics::request(&self.prefix, &self.id),
                QoS::AtLeastOnce,
                false,
                payload,
            )
            .await
            .unwrap();
    }

    /// The next message on `topic`. Messages on other topics are kept for later.
    async fn next_on(&mut self, topic: &str) -> Vec<u8> {
        if let Some(i) = self.buffered.iter().position(|(t, _)| t == topic) {
            return self.buffered.remove(i).1;
        }
        loop {
            let (t, payload) = timeout(WAIT, self.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("no message on {topic} in time"))
                .unwrap();
            if t == topic {
                return payload;
            }
            self.buffered.push((t, payload));
        }
    }

    async fn response(&mut self) -> ResponseEnvelope {
        let topic = topics::response(&self.prefix, &self.id);
        serde_json::from_slice(&self.next_on(&topic).await).unwrap()
    }

    async fn event(&mut self) -> ChangeEvent {
        let topic = topics::events(&self.prefix);
        serde_json::from_slice(&self.next_on(&topic).await).unwrap()
    }
}

fn record(result: RpcResult) -> RecordRow {
    match result {
        RpcResult::Ok {
            data: RpcData::Record { record },
        } => record,
        other => panic!("expected a record, got {other:?}"),
    }
}

async fn event_ops(pool: &PgPool, id: i64) -> Vec<(String, String)> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT op::text, connection_type::text FROM record_events WHERE record_id = $1 ORDER BY time",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[sqlx::test]
async fn rest_crud_health_and_config(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    let http = &app.http;

    // Health is public: no cookie needed.
    let health: Health = app
        .anon
        .get(app.url("/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (health.status.as_str(), health.db, health.mqtt),
        ("ok", true, true)
    );

    // config.json (the browser's MQTT settings) is gone with browser MQTT.
    assert_eq!(
        http.get(app.url("/config.json"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // Create
    let res = http
        .post(app.url("/api/records"))
        .json(&json!({ "value": "hello" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let created: RecordRow = res.json().await.unwrap();
    assert_eq!(created.value.as_deref(), Some("hello"));
    assert_eq!(created.connection_type, ConnectionType::Https);

    // List
    let list: Vec<RecordRow> = http
        .get(app.url("/api/records"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list, vec![created.clone()]);

    // Update (to null)
    let res = http
        .put(app.url(&format!("/api/records/{}", created.id)))
        .json(&json!({ "value": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let updated: RecordRow = res.json().await.unwrap();
    assert_eq!(updated.value, None);
    assert!(updated.updated_at > created.updated_at);

    // Delete, then the record is gone
    let path = app.url(&format!("/api/records/{}", created.id));
    assert_eq!(http.delete(&path).send().await.unwrap().status(), 204);
    assert_eq!(http.delete(&path).send().await.unwrap().status(), 404);
    let res = http
        .put(&path)
        .json(&json!({ "value": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["code"],
        "not_found"
    );

    // Validation
    let res = http
        .post(app.url("/api/records"))
        .json(&json!({ "value": "a".repeat(1001) }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // Every change is in the hypertable log
    assert_eq!(
        event_ops(&pool, created.id).await,
        pairs(&[
            ("create", "https"),
            ("update", "https"),
            ("delete", "https")
        ])
    );

    app.stop().await;
}

#[sqlx::test]
async fn wss_crud_and_live_events(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    let mut a = app.ws().await;
    let mut b = app.ws().await;

    a.send(
        "c1",
        Request::Create {
            value: Some("ws".into()),
        },
    )
    .await;
    let created = record(a.response("c1").await);
    assert_eq!(created.connection_type, ConnectionType::Wss);
    // The other client sees the change live.
    assert_eq!(
        b.event().await,
        ChangeEvent::Create {
            record: created.clone()
        }
    );

    a.send(
        "u1",
        Request::Update {
            id: created.id,
            value: Some("ws2".into()),
        },
    )
    .await;
    let updated = record(a.response("u1").await);
    assert_eq!(b.event().await, ChangeEvent::Update { record: updated });

    a.send("l1", Request::List).await;
    match a.response("l1").await {
        RpcResult::Ok {
            data: RpcData::Records { records },
        } => assert_eq!(records.len(), 1),
        other => panic!("{other:?}"),
    }

    a.send("d1", Request::Delete { id: created.id }).await;
    assert!(matches!(
        a.response("d1").await,
        RpcResult::Ok {
            data: RpcData::Deleted { .. }
        }
    ));
    assert_eq!(
        b.event().await,
        ChangeEvent::Delete {
            id: created.id,
            connection_type: ConnectionType::Wss
        }
    );

    // A missing record and a malformed message are reported, not fatal.
    a.send("d2", Request::Delete { id: created.id }).await;
    assert!(matches!(a.response("d2").await, RpcResult::Error { .. }));
    a.0.send(Message::Text(
        r#"{"request_id":"bad","request":{"op":"nope"}}"#.into(),
    ))
    .await
    .unwrap();
    assert!(matches!(a.response("bad").await, RpcResult::Error { .. }));

    // A change over REST also reaches WebSocket clients.
    let rest: RecordRow = app
        .http
        .post(app.url("/api/records"))
        .json(&json!({ "value": "from rest" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(b.event().await, ChangeEvent::Create { record: rest });

    assert_eq!(
        event_ops(&pool, created.id).await,
        pairs(&[("create", "wss"), ("update", "wss"), ("delete", "wss")])
    );
    app.stop().await;
}

#[sqlx::test]
async fn mqtt_crud_and_live_events(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    let mut mqtt = Mqtt::connect(&app.prefix).await;
    let mut ws = app.ws().await;

    // The server's retained status.
    let status = mqtt.next_on(&topics::server_status(&app.prefix)).await;
    assert_eq!(status, b"online");

    mqtt.send("c1", Request::Create { value: None }).await;
    let res = mqtt.response().await;
    assert_eq!(res.request_id, "c1");
    let created = record(res.result);
    assert_eq!(created.connection_type, ConnectionType::Mqtt);
    // Broadcast on the MQTT events topic and over the WebSocket.
    assert_eq!(
        mqtt.event().await,
        ChangeEvent::Create {
            record: created.clone()
        }
    );
    assert_eq!(
        ws.event().await,
        ChangeEvent::Create {
            record: created.clone()
        }
    );

    mqtt.send(
        "u1",
        Request::Update {
            id: created.id,
            value: Some("m".into()),
        },
    )
    .await;
    let updated = record(mqtt.response().await.result);
    assert_eq!(updated.value.as_deref(), Some("m"));
    assert_eq!(mqtt.event().await, ChangeEvent::Update { record: updated });

    mqtt.send("l1", Request::List).await;
    assert!(matches!(
        mqtt.response().await.result,
        RpcResult::Ok { data: RpcData::Records { records } } if records.len() == 1
    ));

    mqtt.send("d1", Request::Delete { id: created.id }).await;
    assert!(matches!(mqtt.response().await.result, RpcResult::Ok { .. }));
    assert_eq!(
        mqtt.event().await,
        ChangeEvent::Delete {
            id: created.id,
            connection_type: ConnectionType::Mqtt
        }
    );

    // Malformed payload: an error response, keeping the request id.
    mqtt.send_raw(br#"{"request_id":"bad","request":{}}"#.to_vec())
        .await;
    let res = mqtt.response().await;
    assert_eq!(res.request_id, "bad");
    assert!(matches!(res.result, RpcResult::Error { .. }));

    // A WebSocket change reaches MQTT clients.
    ws.send(
        "w1",
        Request::Create {
            value: Some("from ws".into()),
        },
    )
    .await;
    let from_ws = record(ws.response("w1").await);
    assert_eq!(mqtt.event().await, ChangeEvent::Create { record: from_ws });

    assert_eq!(
        event_ops(&pool, created.id).await,
        pairs(&[("create", "mqtt"), ("update", "mqtt"), ("delete", "mqtt")])
    );

    // On a clean shutdown the server marks itself offline.
    let status_topic = topics::server_status(&app.prefix);
    app.stop().await;
    assert_eq!(mqtt.next_on(&status_topic).await, b"offline");
}

#[sqlx::test]
async fn serves_client_files_under_prefix(pool: PgPool) {
    let dir = std::env::temp_dir().join(format!("probe-static-{:016x}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html>index").unwrap();
    std::fs::write(dir.join("app.js"), "console.log(1)").unwrap();
    let app = TestApp::start_with(pool, Some(dir.to_string_lossy().into_owned())).await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let get = |path: &str| http.get(format!("{}{path}", app.base)).send();

    let res = get("/probe").await.unwrap();
    assert_eq!(res.status(), 308);
    assert_eq!(res.headers()["location"], "/probe/");

    for path in ["/probe/", "/probe/some/client/route"] {
        let res = get(path).await.unwrap();
        assert_eq!(res.status(), 200, "{path}");
        assert_eq!(res.text().await.unwrap(), "<!doctype html>index", "{path}");
    }
    assert_eq!(
        get("/probe/app.js").await.unwrap().text().await.unwrap(),
        "console.log(1)"
    );
    assert_eq!(get("/probe/api/nope").await.unwrap().status(), 404);
    assert_eq!(get("/elsewhere").await.unwrap().status(), 404);

    app.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Login and sessions -------------------------------------------------------

#[sqlx::test]
async fn login_me_and_logout(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;

    // The login set a cookie with all the protective attributes.
    let res = app.login("Tester", PASSWORD).await; // usernames ignore case
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-store");
    let set = set_cookie(&res).unwrap();
    for attr in [
        "Path=/probe",
        "HttpOnly",
        "Secure",
        "SameSite=Strict",
        "Max-Age=2592000",
    ] {
        assert!(set.contains(attr), "{set} lacks {attr}");
    }
    let me: serde_json::Value = res.json().await.unwrap();
    assert_eq!(me, json!({ "username": "tester" }));

    // The database has only the SHA-256 of the token, never the token.
    let token = app.cookie.split_once('=').unwrap().1;
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE id_hash = sha256(convert_to($1, 'UTF8'))",
    )
    .bind(token)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, 1);

    let res = app.http.get(app.url("/api/auth/me")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-store");

    // Logout deletes the session; the same cookie is worthless afterwards.
    let res = app
        .http
        .post(app.url("/api/auth/logout"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(set_cookie(&res).unwrap().contains("Max-Age=0"));
    let res = app.http.get(app.url("/api/auth/me")).send().await.unwrap();
    assert_eq!(res.status(), 401);
    // ...and the browser is told to drop it.
    assert!(set_cookie(&res).unwrap().contains("Max-Age=0"));
    assert_eq!(
        app.http
            .get(app.url("/api/records"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        ws_refused(app.ws_with(Some(&app.cookie), Some(ORIGIN)).await),
        401
    );

    // Logging out again is harmless.
    let res = app
        .http
        .post(app.url("/api/auth/logout"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    app.stop().await;
}

#[sqlx::test]
async fn wrong_password_and_unknown_user(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    for (user, password) in [
        (USER, "wrong password!"),
        ("nobody", PASSWORD),
        ("not a valid name", PASSWORD),
        (USER, ""),
    ] {
        let res = app.login(user, password).await;
        assert_eq!(res.status(), 401, "{user}");
        assert!(set_cookie(&res).is_none());
        let body: serde_json::Value = res.json().await.unwrap();
        // The same answer whether the user exists or not.
        assert_eq!(
            body,
            json!({ "code": "unauthorized", "message": "wrong username or password" })
        );
    }
    app.stop().await;
}

#[sqlx::test]
async fn protected_routes_need_a_session(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    let forged = format!("{COOKIE}={}", "ab".repeat(32));
    let malformed = format!("{COOKIE}=nope");
    let twice = format!("{0}; {0}", app.cookie);

    for cookie in [None, Some(forged.as_str()), Some(&malformed), Some(&twice)] {
        let c = client(cookie, Some(ORIGIN));
        for path in ["/api/records", "/api/auth/me"] {
            let res = c.get(app.url(path)).send().await.unwrap();
            assert_eq!(res.status(), 401, "{path} with {cookie:?}");
            let body: serde_json::Value = res.json().await.unwrap();
            assert_eq!(body["code"], "unauthorized");
        }
        let res = c
            .post(app.url("/api/records"))
            .json(&json!({ "value": "x" }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
        let res = c
            .post(app.url("/api/auth/logout-all"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
        assert_eq!(ws_refused(app.ws_with(cookie, Some(ORIGIN)).await), 401);
    }
    // Nothing was created by the refused requests.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM records")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);

    // Health is public.
    let res = app.anon.get(app.url("/health")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    app.stop().await;
}

#[sqlx::test]
async fn expired_and_idle_sessions_are_rejected(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    let me = |cookie: &str| {
        let c = client(Some(cookie), None);
        let url = app.url("/api/auth/me");
        async move { c.get(url).send().await.unwrap().status() }
    };

    // Absolute timeout.
    let a = session_cookie(&app.login(USER, PASSWORD).await);
    assert_eq!(me(&a).await, 200);
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(me(&a).await, 401);
    assert_eq!(me(&app.cookie).await, 401);

    // Idle timeout (7 days without a request), though not expired yet.
    let b = session_cookie(&app.login(USER, PASSWORD).await);
    sqlx::query(
        "UPDATE sessions SET last_seen_at = now() - interval '7 days 1 minute'
         WHERE expires_at > now()",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(me(&b).await, 401);

    // Sliding: a request moves last_seen_at forward (at most once a minute).
    let c = session_cookie(&app.login(USER, PASSWORD).await);
    sqlx::query(
        "UPDATE sessions SET last_seen_at = now() - interval '6 days'
         WHERE expires_at > now() AND last_seen_at > now() - interval '1 hour'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(me(&c).await, 200);
    let touched: bool = sqlx::query_scalar(
        "SELECT last_seen_at > now() - interval '1 minute' FROM sessions
         WHERE id_hash = sha256(convert_to($1, 'UTF8'))",
    )
    .bind(c.split_once('=').unwrap().1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(touched);
    app.stop().await;
}

#[sqlx::test]
async fn foreign_origins_are_rejected(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;

    for origin in [
        Some("https://evil.example"),
        Some("http://probe.test.evil.example"),
        Some("null"),
        None,
    ] {
        // Logged in, but the request comes from another site's page (CSRF).
        let c = client(Some(&app.cookie), origin);
        let res = c
            .post(app.url("/api/records"))
            .json(&json!({ "value": "csrf" }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{origin:?}");
        assert_eq!(
            res.json::<serde_json::Value>().await.unwrap()["code"],
            "forbidden"
        );
        let res = c.delete(app.url("/api/records/1")).send().await.unwrap();
        assert_eq!(res.status(), 403);
        // Login CSRF (logging the victim into the attacker's account) too.
        let res = client(None, origin)
            .post(app.url("/api/auth/login"))
            .json(&json!({ "username": USER, "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403);
        // Cross-site WebSocket hijacking.
        assert_eq!(
            ws_refused(app.ws_with(Some(&app.cookie), origin).await),
            403
        );
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM records")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);

    // Reading needs no Origin (browsers don't always send it on GET), and
    // every origin in ALLOWED_ORIGINS works.
    let res = client(Some(&app.cookie), None)
        .get(app.url("/api/records"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = client(Some(&app.cookie), Some("http://other.allowed.test"))
        .post(app.url("/api/records"))
        .json(&json!({ "value": "ok" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let mut ws = app.ws().await;
    ws.send("l1", Request::List).await;
    assert!(matches!(ws.response("l1").await, RpcResult::Ok { .. }));
    app.stop().await;
}

#[sqlx::test]
async fn failed_logins_are_rate_limited(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    // Five free failures, then the sixth makes the username wait 1 s.
    for _ in 0..6 {
        assert_eq!(app.login(USER, "wrong password!").await.status(), 401);
    }
    // While waiting, even the right password gets 429 (it isn't checked).
    let res = app.login(USER, PASSWORD).await;
    assert_eq!(res.status(), 429);
    assert_eq!(res.headers()["retry-after"], "1");
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["code"],
        "too_many_requests"
    );
    // Upper case doesn't get around it.
    assert_eq!(app.login("TESTER", PASSWORD).await.status(), 429);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(app.login(USER, PASSWORD).await.status(), 200);
    // A success resets the count: five free failures again.
    for _ in 0..5 {
        assert_eq!(app.login(USER, "wrong password!").await.status(), 401);
    }
    assert_eq!(app.login(USER, PASSWORD).await.status(), 200);
    app.stop().await;
}

#[sqlx::test]
async fn logout_everywhere_and_password_change(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    let phone = session_cookie(&app.login(USER, PASSWORD).await);
    let me = |cookie: &str| {
        let c = client(Some(cookie), Some(ORIGIN));
        let url = app.url("/api/auth/me");
        async move { c.get(url).send().await.unwrap().status() }
    };
    assert_eq!(me(&phone).await, 200);

    // "Log out everywhere" from the PC ends the phone's session too.
    let res = app
        .http
        .post(app.url("/api/auth/logout-all"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(me(&app.cookie).await, 401);
    assert_eq!(me(&phone).await, 401);

    // A new password (the set-password command) ends all sessions as well,
    // and only the new password works afterwards.
    let pc = session_cookie(&app.login(USER, PASSWORD).await);
    probe_server::auth::set_password(&pool, USER, "a brand new password")
        .await
        .unwrap();
    assert_eq!(me(&pc).await, 401);
    assert_eq!(app.login(USER, PASSWORD).await.status(), 401);
    assert_eq!(app.login(USER, "a brand new password").await.status(), 200);

    // A login replaces the session sent along with it (no session fixation).
    let first = session_cookie(&app.login(USER, "a brand new password").await);
    let res = client(Some(&first), Some(ORIGIN))
        .post(app.url("/api/auth/login"))
        .json(&json!({ "username": USER, "password": "a brand new password" }))
        .send()
        .await
        .unwrap();
    let second = session_cookie(&res);
    assert_ne!(first, second);
    assert_eq!(me(&first).await, 401);
    assert_eq!(me(&second).await, 200);
    app.stop().await;
}

#[sqlx::test]
async fn user_commands(pool: PgPool) {
    use probe_server::auth::{create_user, delete_user, set_password};
    create_user(&pool, "Alice", "twelve chars").await.unwrap();
    let (name, hash): (String, String) =
        sqlx::query_as("SELECT username, password_hash FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(name, "alice");
    assert!(hash.starts_with("$argon2id$"), "{hash}");

    let err = create_user(&pool, "alice", "twelve chars")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    assert!(create_user(&pool, "bob", "short").await.is_err());
    assert!(
        create_user(&pool, "bad name", "twelve chars")
            .await
            .is_err()
    );
    assert!(set_password(&pool, "nobody", "twelve chars").await.is_err());
    delete_user(&pool, "ALICE").await.unwrap();
    assert!(delete_user(&pool, "alice").await.is_err());
}

// ---- Who created a record --------------------------------------------------------

#[sqlx::test]
async fn records_remember_the_creator_ip(pool: PgPool) {
    let app = TestApp::start(pool.clone()).await;
    let ip_of = |id: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT op::text, host(ip) FROM record_events WHERE record_id = $1 ORDER BY time",
            )
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };

    // REST without a proxy: the TCP peer.
    let direct: RecordRow = app
        .http
        .post(app.url("/api/records"))
        .json(&json!({ "value": "direct" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(direct.created_ip.as_deref(), Some("127.0.0.1"));

    // REST behind Caddy: the last X-Forwarded-For entry (what Caddy saw).
    let proxied: RecordRow = app
        .http
        .post(app.url("/api/records"))
        .header("x-forwarded-for", "198.51.100.1, 203.0.113.7")
        .json(&json!({ "value": "proxied" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(proxied.created_ip.as_deref(), Some("203.0.113.7"));

    // An update by someone else keeps the creator's IP in the record, and
    // the event log has who did what.
    let updated: RecordRow = app
        .http
        .put(app.url(&format!("/api/records/{}", proxied.id)))
        .header("x-forwarded-for", "2001:db8::5")
        .json(&json!({ "value": "changed" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated.created_ip.as_deref(), Some("203.0.113.7"));
    assert_eq!(
        ip_of(proxied.id).await,
        vec![
            ("create".to_string(), Some("203.0.113.7".to_string())),
            ("update".to_string(), Some("2001:db8::5".to_string())),
        ]
    );

    // WSS: the IP of the handshake.
    let mut ws = app.ws().await;
    ws.send(
        "c1",
        Request::Create {
            value: Some("ws".into()),
        },
    )
    .await;
    let over_ws = record(ws.response("c1").await);
    assert_eq!(over_ws.created_ip.as_deref(), Some("127.0.0.1"));

    // MQTT: unknown (the broker doesn't pass on the client's address).
    let mut mqtt = Mqtt::connect(&app.prefix).await;
    mqtt.send("m1", Request::Create { value: None }).await;
    let over_mqtt = record(mqtt.response().await.result);
    assert_eq!(over_mqtt.created_ip, None);
    assert_eq!(
        ip_of(over_mqtt.id).await,
        vec![("create".to_string(), None)]
    );

    // The list carries it too.
    let list: Vec<RecordRow> = app
        .http
        .get(app.url("/api/records"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ips: Vec<Option<&str>> = list.iter().map(|r| r.created_ip.as_deref()).collect();
    assert_eq!(
        ips,
        [
            None,
            Some("127.0.0.1"),
            Some("203.0.113.7"),
            Some("127.0.0.1")
        ]
    );
    app.stop().await;
}
