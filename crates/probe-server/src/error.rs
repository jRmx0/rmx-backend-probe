use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use probe_protocol::{ApiError, ErrorCode};

/// Errors from the records service. Each transport turns them into its own
/// error format: an HTTP status + JSON body, or an `RpcResult::Error`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("record {0} not found")]
    NotFound(i64),
    #[error("database error")]
    Db(#[from] sqlx::Error),
    #[error("login required")]
    Unauthorized,
    #[error("wrong username or password")]
    InvalidCredentials,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("too many failed logins, try again in {retry_after} s")]
    TooManyRequests { retry_after: u64 },
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}

impl AppError {
    pub fn to_api(&self) -> ApiError {
        let code = match self {
            Self::BadRequest(_) => ErrorCode::BadRequest,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Db(_) | Self::Internal(_) => ErrorCode::Internal,
            Self::Unauthorized | Self::InvalidCredentials => ErrorCode::Unauthorized,
            Self::Forbidden(_) => ErrorCode::Forbidden,
            Self::TooManyRequests { .. } => ErrorCode::TooManyRequests,
        };
        // Database details stay in the server log, not in the response.
        ApiError {
            code,
            message: self.to_string(),
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Db(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unauthorized | Self::InvalidCredentials => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
        }
    }

    pub fn log(&self) {
        match self {
            Self::Db(e) => tracing::error!(error = %e, "database error"),
            Self::Internal(e) => tracing::error!(error = %e, "internal error"),
            _ => {}
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.log();
        let mut res = (self.status(), Json(self.to_api())).into_response();
        if let Self::TooManyRequests { retry_after } = self {
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_errors_hide_details() {
        let e = AppError::Db(sqlx::Error::PoolTimedOut);
        let api = e.to_api();
        assert_eq!(api.code, ErrorCode::Internal);
        assert_eq!(api.message, "database error");
        assert_eq!(e.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(AppError::NotFound(3).to_api().message, "record 3 not found");
    }

    #[test]
    fn auth_errors() {
        assert_eq!(AppError::Unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(AppError::Forbidden("x").status(), StatusCode::FORBIDDEN);
        let res = AppError::TooManyRequests { retry_after: 4 }.into_response();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(res.headers()[header::RETRY_AFTER], "4");
        let e = AppError::Internal(anyhow::anyhow!("secret detail"));
        assert_eq!(e.to_api().message, "internal error");
    }
}
