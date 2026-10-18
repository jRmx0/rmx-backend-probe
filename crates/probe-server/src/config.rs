//! Configuration, read only from environment variables.
//! Locally, `main` loads a `.env` file first (dotenvy); in a container the
//! variables come from Compose.

use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, bail};

#[derive(Clone, Debug)]
pub struct Config {
    /// `BIND_ADDR`, default `0.0.0.0:8080`.
    pub bind_addr: SocketAddr,
    /// `DATABASE_URL`, required.
    pub database_url: String,
    /// `DB_MAX_CONNECTIONS`, default 5.
    pub db_max_connections: u32,
    pub mqtt: MqttConfig,
    /// `MQTT_WS_URL`: the broker's WebSocket URL as the *browser* sees it.
    /// Served in `/probe/config.json`. Empty if unset.
    pub mqtt_ws_url: String,
    /// `STATIC_DIR`: built client files to serve under `/probe/`. Unset in local
    /// development, where the Bun dev server serves the client instead.
    pub static_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct MqttConfig {
    /// `MQTT_HOST`, required.
    pub host: String,
    /// `MQTT_PORT`, default 1883.
    pub port: u16,
    /// `MQTT_USERNAME`, required.
    pub username: String,
    /// `MQTT_PASSWORD`, required. Never logged.
    pub password: String,
    /// `MQTT_CLIENT_ID`, default `probe-server`.
    pub client_id: String,
    /// `MQTT_TOPIC_PREFIX`, default `probe`.
    pub topic_prefix: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads settings through `get`, so tests can pass a map instead of the real environment.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let get = |key: &str| get(key).filter(|v| !v.trim().is_empty());
        let required = |key: &str| get(key).with_context(|| format!("{key} is not set"));

        let bind_addr = get("BIND_ADDR")
            .unwrap_or_else(|| "0.0.0.0:8080".into())
            .parse()
            .context("BIND_ADDR must look like 0.0.0.0:8080")?;
        let db_max_connections = parse_or(get("DB_MAX_CONNECTIONS"), 5, "DB_MAX_CONNECTIONS")?;
        let port = parse_or(get("MQTT_PORT"), 1883, "MQTT_PORT")?;

        let topic_prefix = get("MQTT_TOPIC_PREFIX").unwrap_or_else(|| "probe".into());
        if topic_prefix.contains(['+', '#']) || topic_prefix.ends_with('/') {
            bail!("MQTT_TOPIC_PREFIX must not contain + or # or end with /");
        }

        Ok(Self {
            bind_addr,
            database_url: required("DATABASE_URL")?,
            db_max_connections,
            mqtt: MqttConfig {
                host: required("MQTT_HOST")?,
                port,
                username: required("MQTT_USERNAME")?,
                password: required("MQTT_PASSWORD")?,
                client_id: get("MQTT_CLIENT_ID").unwrap_or_else(|| "probe-server".into()),
                topic_prefix,
            },
            mqtt_ws_url: get("MQTT_WS_URL").unwrap_or_default(),
            static_dir: get("STATIC_DIR").map(PathBuf::from),
        })
    }
}

fn parse_or<T: std::str::FromStr>(
    value: Option<String>,
    default: T,
    key: &str,
) -> anyhow::Result<T> {
    match value {
        None => Ok(default),
        Some(v) => v
            .parse()
            .map_err(|_| anyhow::anyhow!("{key} is not a valid number: {v}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn base() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("DATABASE_URL", "postgres://u:p@db/probe"),
            ("MQTT_HOST", "mosquitto"),
            ("MQTT_USERNAME", "probe-app"),
            ("MQTT_PASSWORD", "secret"),
        ])
    }

    fn load(vars: &HashMap<&str, &str>) -> anyhow::Result<Config> {
        Config::from_lookup(|k| vars.get(k).map(|v| v.to_string()))
    }

    #[test]
    fn defaults() {
        let c = load(&base()).unwrap();
        assert_eq!(c.bind_addr.to_string(), "0.0.0.0:8080");
        assert_eq!(c.mqtt.port, 1883);
        assert_eq!(c.mqtt.client_id, "probe-server");
        assert_eq!(c.mqtt.topic_prefix, "probe");
        assert_eq!(c.db_max_connections, 5);
        assert!(c.static_dir.is_none());
        assert_eq!(c.mqtt_ws_url, "");
    }

    #[test]
    fn missing_required() {
        let mut vars = base();
        vars.remove("MQTT_PASSWORD");
        let err = load(&vars).unwrap_err().to_string();
        assert!(err.contains("MQTT_PASSWORD"), "{err}");

        // An empty value counts as missing.
        vars.insert("MQTT_PASSWORD", " ");
        assert!(load(&vars).is_err());
    }

    #[test]
    fn invalid_values() {
        let mut vars = base();
        vars.insert("MQTT_PORT", "abc");
        assert!(load(&vars).is_err());

        let mut vars = base();
        vars.insert("MQTT_TOPIC_PREFIX", "probe/#");
        assert!(load(&vars).is_err());

        let mut vars = base();
        vars.insert("BIND_ADDR", "8080");
        assert!(load(&vars).is_err());
    }
}
