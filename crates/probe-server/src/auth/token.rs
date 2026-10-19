//! The session cookie: a random token, its hash, and the `Set-Cookie` /
//! `Cookie` header handling.

use axum::http::{HeaderMap, HeaderValue, header};
use sha2::{Digest, Sha256};

/// The cookie's name.
///
/// `__Secure-`: browsers accept this cookie only with `Secure` and only from
/// an HTTPS page, so a plain-HTTP response can never set or overwrite it.
/// The stronger `__Host-` prefix would also forbid `Domain` (no cookie set by
/// a sibling like `rmx.lt` could shadow it), but it requires `Path=/`, and the
/// probe shares `api.rmx.lt` with the future production backend, so its
/// cookie stays under `/probe`. A cookie of our own name sent twice is
/// rejected (see [`session_cookie`]).
pub const COOKIE_NAME: &str = "__Secure-probe_session";

/// The cookie is sent only to URLs under this path.
pub const COOKIE_PATH: &str = "/probe";

/// 32 random bytes = 256 bits, written as 64 lower-case hex characters.
const TOKEN_BYTES: usize = 32;

/// The secret in the cookie. Whoever has it is logged in, so it is never
/// logged, and only its hash goes into the database.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(String);

impl SessionToken {
    /// A new token from the operating system's random number generator.
    pub fn generate() -> Self {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).expect("the OS random number generator failed");
        Self(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Accepts only the exact shape we generate, so junk never reaches the database.
    pub fn parse(value: &str) -> Option<Self> {
        let ok = value.len() == TOKEN_BYTES * 2
            && value
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        ok.then(|| Self(value.to_owned()))
    }

    /// What the database stores and looks up: SHA-256 of the token. A fast
    /// hash is enough because the token is random (no dictionary to try),
    /// unlike passwords, which need a slow hash.
    pub fn hash(&self) -> Vec<u8> {
        Sha256::digest(self.0.as_bytes()).to_vec()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionToken(..)")
    }
}

/// What the request's cookies say about our session cookie.
#[derive(Debug, PartialEq, Eq)]
pub enum CookieLookup {
    Missing,
    /// Exactly one cookie with our name and a well-formed value.
    Found(SessionToken),
    /// Malformed, or sent more than once. More than once can only happen if
    /// some other site under rmx.lt set a cookie with our name (`Domain=` or
    /// another path): rather than guess which one is ours (and maybe log the
    /// user into someone else's account), treat the request as logged out.
    Invalid,
}

/// Finds our session cookie in all `Cookie` headers (HTTP/2 may send several).
pub fn session_cookie(headers: &HeaderMap) -> CookieLookup {
    let mut found = None;
    let mut count = 0;
    for value in headers.get_all(header::COOKIE) {
        let Ok(value) = value.to_str() else { continue };
        for pair in value.split(';') {
            let Some((name, val)) = pair.trim().split_once('=') else {
                continue;
            };
            if name == COOKIE_NAME {
                count += 1;
                found = SessionToken::parse(val.trim_matches('"'));
            }
        }
    }
    match (count, found) {
        (0, _) => CookieLookup::Missing,
        (1, Some(token)) => CookieLookup::Found(token),
        _ => CookieLookup::Invalid,
    }
}

/// `Set-Cookie` for a new session. Each attribute and why:
/// - `HttpOnly`: page JavaScript can't read it, so an XSS bug can't steal it.
/// - `Secure`: sent over HTTPS only (required by the `__Secure-` prefix).
/// - `SameSite=Strict`: never sent on requests started by another site, which
///   blocks CSRF in all current browsers. Lax would also send it when someone
///   follows a link here from another site; we don't need that, because the
///   page itself is public and only its own API calls (same-site) need it.
/// - `Path=/probe`: not sent to the rest of api.rmx.lt.
/// - `Max-Age`: the browser keeps it until the absolute timeout; the server
///   enforces the idle timeout on its own.
pub fn set_cookie(token: &SessionToken, max_age_secs: u64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={}; Path={COOKIE_PATH}; Max-Age={max_age_secs}; HttpOnly; Secure; SameSite=Strict",
        token.as_str()
    ))
    .expect("cookie is ASCII")
}

/// `Set-Cookie` that makes the browser delete the cookie.
pub fn clear_cookie() -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}=; Path={COOKIE_PATH}; Max-Age=0; HttpOnly; Secure; SameSite=Strict"
    ))
    .expect("cookie is ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(cookies: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for c in cookies {
            h.append(header::COOKIE, HeaderValue::from_str(c).unwrap());
        }
        h
    }

    #[test]
    fn tokens_are_random_and_well_formed() {
        let a = SessionToken::generate();
        let b = SessionToken::generate();
        assert_ne!(a, b);
        assert_eq!(a.as_str().len(), 64);
        assert_eq!(SessionToken::parse(a.as_str()), Some(a.clone()));
        assert_eq!(a.hash().len(), 32);
        assert_ne!(a.hash(), a.as_str().as_bytes());
        assert!(!format!("{a:?}").contains(a.as_str()));
    }

    #[test]
    fn parse_rejects_other_shapes() {
        assert!(SessionToken::parse("").is_none());
        assert!(SessionToken::parse(&"a".repeat(63)).is_none());
        assert!(SessionToken::parse(&"A".repeat(64)).is_none());
        assert!(SessionToken::parse(&"g".repeat(64)).is_none());
        assert!(SessionToken::parse(&format!("{}'", "a".repeat(63))).is_none());
    }

    #[test]
    fn finds_the_session_cookie() {
        let t = SessionToken::generate();
        let one = format!("other=1; {COOKIE_NAME}={}; x=y", t.as_str());
        assert_eq!(
            session_cookie(&headers(&[&one])),
            CookieLookup::Found(t.clone())
        );
        // Split over two Cookie headers (HTTP/2).
        let split = format!("{COOKIE_NAME}={}", t.as_str());
        assert_eq!(
            session_cookie(&headers(&["a=b", &split])),
            CookieLookup::Found(t.clone())
        );
        assert_eq!(session_cookie(&headers(&[])), CookieLookup::Missing);
        assert_eq!(
            session_cookie(&headers(&["probe_session=x"])),
            CookieLookup::Missing
        );
    }

    #[test]
    fn duplicate_or_malformed_cookie_is_invalid() {
        let t = SessionToken::generate();
        let twice = format!("{COOKIE_NAME}={0}; {COOKIE_NAME}={0}", t.as_str());
        assert_eq!(session_cookie(&headers(&[&twice])), CookieLookup::Invalid);
        let bad = format!("{COOKIE_NAME}=nope");
        assert_eq!(session_cookie(&headers(&[&bad])), CookieLookup::Invalid);
    }

    #[test]
    fn cookie_attributes() {
        let t = SessionToken::generate();
        let set = set_cookie(&t, 60);
        let set = set.to_str().unwrap();
        assert!(set.starts_with(&format!("{COOKIE_NAME}={};", t.as_str())));
        for attr in [
            "Path=/probe",
            "Max-Age=60",
            "HttpOnly",
            "Secure",
            "SameSite=Strict",
        ] {
            assert!(set.contains(attr), "{set} lacks {attr}");
        }
        assert!(clear_cookie().to_str().unwrap().contains("Max-Age=0"));
    }
}
