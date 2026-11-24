use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Every failure carries a stable machine-readable `code` alongside the human `message`,
/// so a client branches on the code instead of pattern-matching prose.
#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    Conflict(String),
    #[allow(dead_code)]
    Forbidden(String),
    /// Response 402 — only ever means "the smart-query allowance and the paid balance are
    /// both empty", never a subscription paywall.
    ///
    /// The code is `out_of_credits` so the bot can answer with the Buy Credits action
    /// rather than parsing prose.
    OutOfCredits(String),
    /// 429. A rate limit is its own answer: the caller did nothing wrong, they have to wait.
    TooManyRequests(String),
    /// Response 502 — the LLM provider failed or timed out.
    ///
    /// Nothing was delivered and nothing was charged; the message is deliberately generic
    /// because the upstream body may contain provider internals.
    LlmUnavailable(String),
    Internal(String),
}

impl AppError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::OutOfCredits(_) => StatusCode::PAYMENT_REQUIRED,
            AppError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            AppError::LlmUnavailable(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AppError::BadRequest(m)
            | AppError::Unauthorized(m)
            | AppError::NotFound(m)
            | AppError::Conflict(m)
            | AppError::Forbidden(m)
            | AppError::OutOfCredits(m)
            | AppError::TooManyRequests(m)
            | AppError::LlmUnavailable(m)
            | AppError::Internal(m) => m,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "bad_request",
            AppError::Unauthorized(_) => "unauthorized",
            AppError::NotFound(_) => "not_found",
            AppError::Conflict(_) => "conflict",
            AppError::Forbidden(_) => "forbidden",
            AppError::OutOfCredits(_) => "out_of_credits",
            AppError::TooManyRequests(_) => "rate_limited",
            AppError::LlmUnavailable(_) => "llm_unavailable",
            AppError::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = json!({
            "status": "error",
            "code": self.code(),
            "message": self.message(),
        });
        (status, Json(body)).into_response()
    }
}

impl From<mongodb::error::Error> for AppError {
    fn from(err: mongodb::error::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}

impl From<mongodb::bson::ser::Error> for AppError {
    fn from(err: mongodb::bson::ser::Error) -> Self {
        AppError::BadRequest(err.to_string())
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(err: jsonwebtoken::errors::Error) -> Self {
        AppError::Unauthorized(err.to_string())
    }
}
