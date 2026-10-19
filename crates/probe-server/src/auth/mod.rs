//! Login with server-side sessions.
//!
//! - **Users** live in the `users` table, with an Argon2id password hash
//!   ([`password`]). There is no sign-up: users are created on the server with
//!   `probe-server create-user <name>` ([`create_user`]).
//! - **Login** (`POST /probe/api/auth/login`) checks the password and creates
//!   a row in `sessions`. The browser gets a random 256-bit token in an
//!   `HttpOnly` cookie ([`token`]); the database keeps only its SHA-256.
//! - **Every protected request** (REST, the WebSocket handshake, config.json)
//!   sends the cookie automatically; [`routes::require_session`] looks the
//!   hash up and checks both timeouts.
//! - **Logout** deletes the row, so the cookie is worthless from then on, even
//!   if someone copied it. "Log out everywhere" deletes all of a user's rows.
//!
//! Why sessions in a cookie, and not basic auth or a token in JavaScript:
//! - Basic auth has no logout, sends the password with every request, and
//!   iOS WebKit doesn't send it on WebSocket handshakes. Cookies are sent there.
//! - A token kept by JavaScript (localStorage, e.g. a JWT) can be stolen by
//!   any script that runs on the page (an XSS bug, a bad dependency). An
//!   `HttpOnly` cookie can't be read by scripts at all.
//! - A session row in the database can be revoked at once (logout, password
//!   change). A signed stateless token stays valid until it expires.

mod password;
mod rate_limit;
pub mod routes;
mod token;

use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use sqlx::PgPool;
use tokio::sync::Semaphore;

use crate::error::AppError;
pub use password::{MAX_PASSWORD_CHARS, MIN_PASSWORD_CHARS, check_password};
use rate_limit::Limiter;
pub use token::{COOKIE_NAME, SessionToken};

/// Idle timeout: a session not used for this long ends.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Absolute timeout: every session ends this long after login, however active.
pub const ABSOLUTE_TIMEOUT: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// `last_seen_at` is written at most this often per session, not on every request.
const TOUCH_AFTER_SECS: f64 = 60.0;

/// Password checks running at once. Each one needs 19 MiB, and probe-app has
/// a 64 MB memory limit; more logins at the same moment wait their turn.
const PARALLEL_HASHES: usize = 1;
/// A login that can't start its password check within this time gets 429.
const HASH_QUEUE_TIMEOUT: Duration = Duration::from_secs(10);

/// The user behind a request, put into the request by `require_session`.
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub user_id: i64,
    pub username: String,
    /// The SHA-256 of this request's session token (for logout and the WebSocket).
    pub session_hash: Vec<u8>,
}

struct Limits {
    by_username: Limiter,
    by_ip: Limiter,
}

/// Login, session lookup and logout. Cheap to clone.
#[derive(Clone)]
pub struct Auth {
    pool: PgPool,
    limits: Arc<Mutex<Limits>>,
    hashing: Arc<Semaphore>,
}

impl Auth {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            limits: Arc::new(Mutex::new(Limits {
                by_username: Limiter::new(rate_limit::PER_USERNAME),
                by_ip: Limiter::new(rate_limit::PER_IP),
            })),
            hashing: Arc::new(Semaphore::new(PARALLEL_HASHES)),
        }
    }

    /// Checks the password and starts a new session.
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        ip: IpAddr,
        user_agent: Option<&str>,
    ) -> Result<(SessionToken, CurrentUser), AppError> {
        let username = password::normalize_username(username);
        let ip_key = ip.to_string();
        self.check_limits(&username, &ip_key)?;

        let plausible = password::check_username(&username).is_ok()
            && password.chars().count() <= MAX_PASSWORD_CHARS;
        let user = if plausible {
            sqlx::query!(
                "SELECT id, password_hash FROM users WHERE username = $1",
                username
            )
            .fetch_optional(&self.pool)
            .await?
        } else {
            None
        };
        let known = user.is_some();
        let (user_id, phc) = user.map_or((0, None), |u| (u.id, Some(u.password_hash)));
        let ok = self.verify(password, phc).await? && known;

        let now = Instant::now();
        {
            let mut limits = self.limits.lock().expect("limits lock");
            if ok {
                limits.by_username.success(&username);
                limits.by_ip.success(&ip_key);
            } else {
                limits.by_username.failure(&username, now);
                limits.by_ip.failure(&ip_key, now);
            }
        }
        if !ok {
            return Err(AppError::InvalidCredentials);
        }

        let token = SessionToken::generate();
        let session_hash = token.hash();
        let user_agent = user_agent.map(|ua| ua.chars().take(200).collect::<String>());
        sqlx::query!(
            "INSERT INTO sessions (id_hash, user_id, expires_at, user_agent)
             VALUES ($1, $2, now() + make_interval(secs => $3), $4)",
            session_hash,
            user_id,
            ABSOLUTE_TIMEOUT.as_secs_f64(),
            user_agent
        )
        .execute(&self.pool)
        .await?;
        Ok((
            token,
            CurrentUser {
                user_id,
                username,
                session_hash,
            },
        ))
    }

    fn check_limits(&self, username: &str, ip_key: &str) -> Result<(), AppError> {
        let limits = self.limits.lock().expect("limits lock");
        let now = Instant::now();
        let wait = [
            limits.by_username.check(username, now),
            limits.by_ip.check(ip_key, now),
        ]
        .into_iter()
        .filter_map(Result::err)
        .max();
        match wait {
            Some(wait) => Err(AppError::TooManyRequests {
                retry_after: wait.as_secs_f64().ceil().max(1.0) as u64,
            }),
            None => Ok(()),
        }
    }

    /// Runs the slow Argon2 check on a blocking thread, one at a time.
    /// Without a stored hash (unknown user) it checks against a dummy hash, so
    /// that takes just as long, and returns false.
    async fn verify(&self, password: &str, phc: Option<String>) -> Result<bool, AppError> {
        let _permit = tokio::time::timeout(HASH_QUEUE_TIMEOUT, self.hashing.acquire())
            .await
            .map_err(|_| AppError::TooManyRequests { retry_after: 1 })?
            .expect("the semaphore is never closed");
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || {
            let phc = phc.as_deref().unwrap_or_else(|| password::dummy_hash());
            password::verify(&password, phc)
        })
        .await
        .map_err(|e| AppError::Internal(e.into()))
    }

    /// The user of a valid session, or `None` if it doesn't exist, has been
    /// idle too long or is past its absolute timeout. With `touch`, this
    /// counts as activity (moves the idle timeout).
    pub async fn validate(
        &self,
        session_hash: &[u8],
        touch: bool,
    ) -> Result<Option<CurrentUser>, sqlx::Error> {
        let row = sqlx::query!(
            r#"SELECT s.user_id, u.username,
                      s.last_seen_at < now() - make_interval(secs => $3) AS "stale!"
               FROM sessions s JOIN users u ON u.id = s.user_id
               WHERE s.id_hash = $1
                 AND s.expires_at > now()
                 AND s.last_seen_at > now() - make_interval(secs => $2)"#,
            session_hash,
            IDLE_TIMEOUT.as_secs_f64(),
            TOUCH_AFTER_SECS
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        if touch && row.stale {
            sqlx::query!(
                "UPDATE sessions SET last_seen_at = now() WHERE id_hash = $1",
                session_hash
            )
            .execute(&self.pool)
            .await?;
        }
        Ok(Some(CurrentUser {
            user_id: row.user_id,
            username: row.username,
            session_hash: session_hash.to_vec(),
        }))
    }

    pub async fn logout(&self, session_hash: &[u8]) -> Result<(), sqlx::Error> {
        sqlx::query!("DELETE FROM sessions WHERE id_hash = $1", session_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Ends every session of the user (all browsers and devices).
    pub async fn logout_everywhere(&self, user_id: i64) -> Result<u64, sqlx::Error> {
        let done = sqlx::query!("DELETE FROM sessions WHERE user_id = $1", user_id)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected())
    }

    /// Deletes sessions past either timeout. They are already rejected; this
    /// only keeps the table small. Runs every hour.
    pub async fn delete_expired(&self) -> Result<u64, sqlx::Error> {
        let done = sqlx::query!(
            "DELETE FROM sessions
             WHERE expires_at <= now() OR last_seen_at <= now() - make_interval(secs => $1)",
            IDLE_TIMEOUT.as_secs_f64()
        )
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }
}

// ---- User management (the `probe-server create-user` etc. commands) --------

/// Creates a user. Fails if the name is taken or the password too short.
pub async fn create_user(pool: &PgPool, username: &str, password: &str) -> anyhow::Result<()> {
    let username = password::normalize_username(username);
    password::check_username(&username).map_err(anyhow::Error::msg)?;
    check_password(password).map_err(anyhow::Error::msg)?;
    let phc = hash_blocking(password).await?;
    let res = sqlx::query!(
        "INSERT INTO users (username, password_hash) VALUES ($1, $2)",
        username,
        phc
    )
    .execute(pool)
    .await;
    match res {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            bail!("user {username} already exists (use set-password to change the password)")
        }
        Err(e) => Err(e).context("creating the user failed"),
    }
}

/// Sets a new password and ends all of the user's sessions.
pub async fn set_password(pool: &PgPool, username: &str, password: &str) -> anyhow::Result<()> {
    let username = password::normalize_username(username);
    check_password(password).map_err(anyhow::Error::msg)?;
    let phc = hash_blocking(password).await?;
    let mut tx = pool.begin().await?;
    let user = sqlx::query_scalar!(
        "UPDATE users SET password_hash = $2, password_changed_at = now()
         WHERE username = $1 RETURNING id",
        username,
        phc
    )
    .fetch_optional(&mut *tx)
    .await?
    .with_context(|| format!("no user {username}"))?;
    sqlx::query!("DELETE FROM sessions WHERE user_id = $1", user)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Deletes a user and (by the foreign key) all their sessions.
pub async fn delete_user(pool: &PgPool, username: &str) -> anyhow::Result<()> {
    let username = password::normalize_username(username);
    let done = sqlx::query!("DELETE FROM users WHERE username = $1", username)
        .execute(pool)
        .await?;
    if done.rows_affected() == 0 {
        bail!("no user {username}");
    }
    Ok(())
}

async fn hash_blocking(password: &str) -> anyhow::Result<String> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || password::hash(&password)).await?
}
