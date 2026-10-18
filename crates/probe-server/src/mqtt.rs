//! MQTT transport (rumqttc, MQTT 3.1.1, plain TCP to the broker on the
//! internal network).
//!
//! - subscribes to `<prefix>/req/+` and answers each request on
//!   `<prefix>/res/<client-id>` (the client id comes from the request topic);
//! - publishes every change event on `<prefix>/events/records`;
//! - keeps a retained `online` on `<prefix>/status/server`, replaced by
//!   `offline` on clean shutdown or, via the last will, when the connection dies.
//!
//! The request/response scheme needs only MQTT 3.1.1 (no v5 response topics),
//! so the same pattern works for small device clients later.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use probe_protocol::{ConnectionType, ResponseEnvelope, topics};
use rumqttc::{AsyncClient, Event, EventLoop, LastWill, MqttOptions, Outgoing, Packet, QoS};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::{config::MqttConfig, records::Records, ws::parse_request};

const ONLINE: &str = "online";
const OFFLINE: &str = "offline";

/// Starts the MQTT client. The returned task ends after a clean disconnect
/// once `shutdown` is cancelled.
pub fn spawn(
    config: &MqttConfig,
    records: Records,
    connected: Arc<AtomicBool>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let prefix = config.topic_prefix.clone();
    let status_topic = topics::server_status(&prefix);

    let mut options = MqttOptions::new(&config.client_id, &config.host, config.port);
    options
        .set_credentials(&config.username, &config.password)
        .set_keep_alive(Duration::from_secs(30))
        .set_clean_session(true)
        // A full list response is up to ~500 records.
        .set_max_packet_size(256 * 1024, 2 * 1024 * 1024)
        .set_last_will(LastWill::new(
            &status_topic,
            OFFLINE,
            QoS::AtLeastOnce,
            true,
        ));
    let (client, eventloop) = AsyncClient::new(options, 64);

    let ctx = Ctx {
        client,
        records,
        prefix,
    };
    tokio::spawn(forward_events(ctx.clone(), shutdown.clone()));
    tokio::spawn(run(ctx, eventloop, connected, shutdown))
}

#[derive(Clone)]
struct Ctx {
    client: AsyncClient,
    records: Records,
    prefix: String,
}

async fn run(
    ctx: Ctx,
    mut eventloop: EventLoop,
    connected: Arc<AtomicBool>,
    shutdown: CancellationToken,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let event = tokio::select! {
            event = eventloop.poll() => event,
            () = shutdown.cancelled() => break,
        };
        match event {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                tracing::info!("mqtt connected");
                backoff = Duration::from_secs(1);
                connected.store(true, Ordering::Relaxed);
                // Clean session: subscriptions must be renewed on every connect.
                let filter = topics::request_filter(&ctx.prefix);
                let status = topics::server_status(&ctx.prefix);
                if let Err(e) = ctx.client.try_subscribe(filter, QoS::AtLeastOnce) {
                    tracing::error!(error = %e, "mqtt subscribe failed");
                }
                if let Err(e) = ctx
                    .client
                    .try_publish(status, QoS::AtLeastOnce, true, ONLINE)
                {
                    tracing::error!(error = %e, "mqtt status publish failed");
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let ctx = ctx.clone();
                tokio::spawn(
                    async move { ctx.handle_request(&publish.topic, &publish.payload).await },
                );
            }
            Ok(_) => {}
            Err(e) => {
                connected.store(false, Ordering::Relaxed);
                tracing::warn!(error = %e, retry_in = ?backoff, "mqtt connection error");
                // The next poll() reconnects. Wait first so a down broker isn't hammered.
                tokio::select! {
                    () = tokio::time::sleep(backoff) => {}
                    () = shutdown.cancelled() => break,
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }

    // Clean shutdown: mark the server offline, then disconnect. The event loop
    // has to keep running for these packets to go out.
    if connected.swap(false, Ordering::Relaxed) {
        let status = topics::server_status(&ctx.prefix);
        let _ = ctx
            .client
            .try_publish(status, QoS::AtLeastOnce, true, OFFLINE);
        let _ = ctx.client.try_disconnect();
        let drain = async {
            loop {
                match eventloop.poll().await {
                    Ok(Event::Outgoing(Outgoing::Disconnect)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        };
        if tokio::time::timeout(Duration::from_secs(3), drain)
            .await
            .is_err()
        {
            tracing::warn!("mqtt disconnect timed out");
        }
    }
    tracing::info!("mqtt stopped");
}

impl Ctx {
    async fn handle_request(&self, topic: &str, payload: &[u8]) {
        let Some(client_id) = topics::parse_request(&self.prefix, topic) else {
            tracing::debug!(topic, "ignoring message on unexpected topic");
            return;
        };
        let response = match parse_request(payload) {
            Ok(env) => ResponseEnvelope {
                request_id: env.request_id,
                result: self.records.handle(ConnectionType::Mqtt, env.request).await,
            },
            Err(response) => response,
        };
        let json = serde_json::to_vec(&response).expect("ResponseEnvelope serializes");
        let topic = topics::response(&self.prefix, client_id);
        if let Err(e) = self
            .client
            .publish(topic, QoS::AtLeastOnce, false, json)
            .await
        {
            tracing::warn!(error = %e, "mqtt response publish failed");
        }
    }
}

/// Publishes every change event (from any transport) to the events topic.
async fn forward_events(ctx: Ctx, shutdown: CancellationToken) {
    let topic = topics::events(&ctx.prefix);
    let mut events = ctx.records.subscribe();
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            () = shutdown.cancelled() => break,
        };
        match event {
            Ok(event) => {
                let json = serde_json::to_vec(&event).expect("ChangeEvent serializes");
                // try_publish: never block here while the broker is down; the
                // event is simply not delivered over MQTT (clients resync on reconnect).
                if let Err(e) = ctx
                    .client
                    .try_publish(&topic, QoS::AtLeastOnce, false, json)
                {
                    tracing::warn!(error = %e, "mqtt event publish failed");
                }
            }
            Err(RecvError::Lagged(n)) => tracing::warn!(missed = n, "mqtt event forwarder lagged"),
            Err(RecvError::Closed) => break,
        }
    }
}
