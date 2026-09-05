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
    #[allow(dead_code)]
    NotImplemented(String),
    PaymentRequired(String),
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
            AppError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            AppError::PaymentRequired(_) => StatusCode::PAYMENT_REQUIRED,
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
            | AppError::NotImplemented(m)
            | AppError::PaymentRequired(m)
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
