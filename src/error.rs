use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    Conflict(String),
    /// 403. Recognised, but not allowed to do this yet — deliberately distinct from 401, so
    /// a client does not sign the user out over a state they can fix by verifying an email.
    Forbidden(String),
    #[allow(dead_code)]
    NotImplemented(String),
    PaymentRequired(String),
    /// 429. Rate limits are their own answer, not a bad request: the caller did nothing
    /// wrong, they simply have to wait, and a client that cannot tell those apart either
    /// retries into the limit or shows the user a validation error they cannot fix.
    TooManyRequests(String),
    BadGateway(String),
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
            AppError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            AppError::PaymentRequired(_) => StatusCode::PAYMENT_REQUIRED,
            AppError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            AppError::BadGateway(_) => StatusCode::BAD_GATEWAY,
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
            | AppError::NotImplemented(m)
            | AppError::PaymentRequired(m)
            | AppError::TooManyRequests(m)
            | AppError::BadGateway(m)
            | AppError::Internal(m) => m,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let message = self.message().to_string();
        (
            status,
            Json(json!({ "status": "error", "message": message })),
        )
            .into_response()
    }
}

impl From<mongodb::error::Error> for AppError {
    fn from(err: mongodb::error::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(err: jsonwebtoken::errors::Error) -> Self {
        AppError::Unauthorized(err.to_string())
    }
}

impl From<mongodb::bson::ser::Error> for AppError {
    fn from(err: mongodb::bson::ser::Error) -> Self {
        AppError::BadRequest(err.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        AppError::BadGateway(err.to_string())
    }
}
