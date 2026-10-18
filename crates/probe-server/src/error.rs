use axum::{
    Json,
    http::StatusCode,
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
}

impl AppError {
    pub fn to_api(&self) -> ApiError {
        let code = match self {
            Self::BadRequest(_) => ErrorCode::BadRequest,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Db(_) => ErrorCode::Internal,
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
            Self::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn log(&self) {
        if let Self::Db(e) = self {
            tracing::error!(error = %e, "database error");
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.log();
        (self.status(), Json(self.to_api())).into_response()
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
}
