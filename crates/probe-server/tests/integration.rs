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

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use probe_protocol::{
    ChangeEvent, ClientConfig, ConnectionType, Health, RecordRow, Request, RequestEnvelope,
    ResponseEnvelope, RpcData, RpcResult, ServerMessage, topics,
};
use probe_server::Config;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::json;
use sqlx::PgPool;
use tokio::{net::TcpStream, sync::mpsc, time::timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

const WAIT: Duration = Duration::from_secs(5);

/// One running probe-server on a random local port.
struct TestApp {
    base: String,
    prefix: String,
    http: reqwest::Client,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl TestApp {
    async fn start(pool: PgPool) -> Self {
        let _ = dotenvy::dotenv();
        let prefix = format!("probe/it-{:016x}", rand::random::<u64>());
        let config = {
            let prefix = prefix.clone();
            Config::from_lookup(move |key| match key {
                "MQTT_TOPIC_PREFIX" => Some(prefix.clone()),
                "MQTT_CLIENT_ID" => Some(format!("server-{}", prefix.replace('/', "-"))),
                "MQTT_WS_URL" => Some("ws://example.test/mqtt".into()),
                "STATIC_DIR" => None,
                _ => std::env::var(key).ok(),
            })
            .expect("test config (is .env present?)")
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(probe_server::serve(
            config,
            pool,
            listener,
            shutdown.clone(),
        ));

        let app = Self {
            base,
            prefix,
            http: reqwest::Client::new(),
            shutdown,
            task,
        };
        app.wait_healthy().await;
        app
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

    async fn ws(&self) -> Ws {
        let url = format!("{}/ws", self.url("").replacen("http", "ws", 1));
        let (stream, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        Ws(stream)
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

/// A browser-like MQTT client logged in as probe-web.
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
        let password = std::env::var("PROBE_WEB_PASSWORD").expect("PROBE_WEB_PASSWORD");
        let mut options = MqttOptions::new(&id, host, port);
        options.set_credentials("probe-web", password);
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

    let health: Health = http
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

    let config: ClientConfig = http
        .get(app.url("/config.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(config.mqtt_topic_prefix, app.prefix);
    assert_eq!(config.mqtt_ws_url, "ws://example.test/mqtt");

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
