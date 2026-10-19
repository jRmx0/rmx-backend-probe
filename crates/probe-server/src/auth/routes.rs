//! HTTP side of the login: the `/probe/api/auth/*` handlers, the session
//! middleware, and the Origin check against cross-site requests.

use axum::{
    Extension, Json,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use probe_protocol::{LoginRequest, Me};

use super::{
    ABSOLUTE_TIMEOUT, CurrentUser,
    token::{CookieLookup, clear_cookie, session_cookie, set_cookie},
};
use crate::{AppState, client_ip::ClientIp, error::AppError};

/// Responses that carry session data must not be stored by any cache.
const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

/// `POST /probe/api/auth/login` with `{"username", "password"}`.
/// 200 + `Set-Cookie` + `Me`; 401 for a wrong username or password (the same
/// answer for both, so it doesn't reveal which names exist); 429 + `Retry-After`
/// after too many failures.
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    // Every login gets a new session id, and an old one sent along ends now.
    // (Reusing an id from before the login would allow "session fixation".)
    if let CookieLookup::Found(old) = session_cookie(&headers) {
        state.auth.logout(&old.hash()).await?;
    }
    let (token, user) = match state
        .auth
        .login(&body.username, &body.password, ip, user_agent)
        .await
    {
        Ok(ok) => ok,
        Err(e) => {
            // No username in the log: people sometimes type a password there.
            tracing::warn!(%ip, error = %e, "login failed");
            return Err(e);
        }
    };
    tracing::info!(username = %user.username, %ip, "login");
    Ok((
        [
            (
                header::SET_COOKIE,
                set_cookie(&token, ABSOLUTE_TIMEOUT.as_secs()),
            ),
            (header::CACHE_CONTROL, NO_STORE),
        ],
        Json(Me {
            username: user.username,
        }),
    )
        .into_response())
}

/// `POST /probe/api/auth/logout`: ends this session. Always 204 and clears the
/// cookie, even if there was no valid session.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let CookieLookup::Found(token) = session_cookie(&headers) {
        state.auth.logout(&token.hash()).await?;
    }
    Ok(logged_out())
}

/// `POST /probe/api/auth/logout-all`: ends every session of this user.
pub async fn logout_everywhere(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Response, AppError> {
    let n = state.auth.logout_everywhere(user.user_id).await?;
    tracing::info!(username = %user.username, sessions = n, "logged out everywhere");
    Ok(logged_out())
}

/// `GET /probe/api/auth/me`: who is logged in (401 if nobody). The client asks
/// this at start to choose between the login form and the app.
pub async fn me(Extension(user): Extension<CurrentUser>) -> Json<Me> {
    Json(Me {
        username: user.username,
    })
}

fn logged_out() -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (header::SET_COOKIE, clear_cookie()),
            (header::CACHE_CONTROL, NO_STORE),
        ],
    )
        .into_response()
}

/// Middleware for every protected route: 401 without a valid session,
/// otherwise puts the [`CurrentUser`] into the request for the handler.
pub async fn require_session(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let lookup = session_cookie(req.headers());
    let user = match &lookup {
        CookieLookup::Found(token) => match state.auth.validate(&token.hash(), true).await {
            Ok(user) => user,
            Err(e) => return AppError::Db(e).into_response(),
        },
        _ => None,
    };
    let Some(user) = user else {
        let mut res = AppError::Unauthorized.into_response();
        // A cookie that no longer works (expired, logged out, forged): tell
        // the browser to drop it.
        if lookup != CookieLookup::Missing {
            res.headers_mut().append(header::SET_COOKIE, clear_cookie());
        }
        return res;
    };
    req.extensions_mut().insert(user);
    let mut res = next.run(req).await;
    res.headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(NO_STORE);
    res
}

/// Middleware for everything under `/probe`: requests that change something
/// (any method but GET/HEAD/OPTIONS) and WebSocket handshakes must come from
/// one of `ALLOWED_ORIGINS`, else 403.
///
/// Why, on top of `SameSite=Strict`: SameSite works per *site* (rmx.lt), so
/// it doesn't stop a page on another rmx.lt subdomain, and older browsers
/// ignore it. Browsers always send `Origin` on these requests and on
/// WebSocket handshakes, and a page can't fake it. WebSockets matter most:
/// they are not covered by CORS, so without this check any website could open
/// one to us with the user's cookie ("cross-site WebSocket hijacking").
/// No `Origin` at all is refused too: no browser omits it here, so only
/// non-browser clients would, and they have no business sending our cookie.
pub async fn check_origin(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if needs_origin_check(req.method(), req.headers())
        && !origin_allowed(req.headers(), &state.config.allowed_origins)
    {
        tracing::warn!(
            origin = ?req.headers().get(header::ORIGIN),
            path = %req.uri().path(),
            "request from a foreign origin refused"
        );
        return AppError::Forbidden("cross-origin request refused").into_response();
    }
    next.run(req).await
}

fn needs_origin_check(method: &Method, headers: &HeaderMap) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
        || headers.contains_key(header::UPGRADE)
}

fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    match (origins.next(), origins.next()) {
        (Some(origin), None) => origin
            .to_str()
            .is_ok_and(|o| allowed.iter().any(|a| a.eq_ignore_ascii_case(o))),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.append(*k, HeaderValue::from_str(v).unwrap());
        }
        map
    }

    #[test]
    fn which_requests_need_an_origin() {
        let none = HeaderMap::new();
        assert!(!needs_origin_check(&Method::GET, &none));
        assert!(!needs_origin_check(&Method::HEAD, &none));
        for m in [
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::CONNECT,
        ] {
            assert!(needs_origin_check(&m, &none), "{m}");
        }
        assert!(needs_origin_check(
            &Method::GET,
            &h(&[("upgrade", "websocket")])
        ));
    }

    #[test]
    fn origin_must_match_exactly_once() {
        let allowed = vec!["https://api.rmx.lt".to_string()];
        assert!(origin_allowed(
            &h(&[("origin", "https://api.rmx.lt")]),
            &allowed
        ));
        assert!(origin_allowed(
            &h(&[("origin", "https://API.rmx.lt")]),
            &allowed
        ));
        for bad in [
            "https://evil.example",
            "https://api.rmx.lt.evil.example",
            "http://api.rmx.lt",
            "https://rmx.lt",
            "https://api.rmx.lt:444",
            "null",
        ] {
            assert!(!origin_allowed(&h(&[("origin", bad)]), &allowed), "{bad}");
        }
        assert!(!origin_allowed(&HeaderMap::new(), &allowed));
        let twice = h(&[
            ("origin", "https://api.rmx.lt"),
            ("origin", "https://api.rmx.lt"),
        ]);
        assert!(!origin_allowed(&twice, &allowed));
    }
}
