//! The client's IP address, for the login rate limit and for recording who
//! changed a record.

use std::{
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, request::Parts},
};

/// Extractor: the client's IP for this request.
///
/// Behind Caddy, the TCP peer is Caddy, so this uses the last
/// `X-Forwarded-For` entry: the address Caddy itself saw (Caddy replaces
/// whatever the client sent, and the app's port is not published, so only
/// Caddy can reach it). Without that header (local development), the TCP peer.
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // `serve` always provides ConnectInfo; 0.0.0.0 only if someone serves
        // the router without it.
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |c| c.0.ip());
        Ok(Self(client_ip(&parts.headers, peer)))
    }
}

fn client_ip(headers: &HeaderMap, peer: IpAddr) -> IpAddr {
    headers
        .get_all("x-forwarded-for")
        .iter()
        .next_back()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|ip| ip.trim().parse().ok())
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        map
    }

    #[test]
    fn client_ip_from_proxy_header() {
        let peer: IpAddr = "172.18.0.2".parse().unwrap();
        assert_eq!(client_ip(&HeaderMap::new(), peer), peer);
        assert_eq!(
            client_ip(&h("203.0.113.7"), peer).to_string(),
            "203.0.113.7"
        );
        // The last entry is the one the nearest proxy added.
        assert_eq!(
            client_ip(&h("1.2.3.4, 203.0.113.7"), peer).to_string(),
            "203.0.113.7"
        );
        assert_eq!(
            client_ip(&h("2001:db8::1"), peer).to_string(),
            "2001:db8::1"
        );
        assert_eq!(client_ip(&h("nope"), peer), peer);
    }
}
