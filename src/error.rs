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
    /// 402 with the numbers a client needs to prompt for a top-up: the chat shows a
    /// button that routes to the account page, and it cannot do that sensibly without
    /// knowing the balance and what the message would have cost.
    InsufficientCredits {
        balance_tokens: u64,
        required_tokens: u64,
    },
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
            AppError::PaymentRequired(_) | AppError::InsufficientCredits { .. } => {
                StatusCode::PAYMENT_REQUIRED
            }
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
            AppError::InsufficientCredits { .. } => {
                "Your credit balance is too low for this message. Please recharge to continue."
            }
        }
    }

    /// Stable, machine-readable reason. Clients branch on this instead of matching the
    /// human message, which is free to be reworded at any time.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "bad_request",
            AppError::Unauthorized(_) => "unauthorized",
            AppError::NotFound(_) => "not_found",
            AppError::Conflict(_) => "conflict",
            AppError::Forbidden(_) => "forbidden",
            AppError::NotImplemented(_) => "not_implemented",
            // A 402 in this service only ever means the balance is short, so both arms
            // tell the client the same thing; the second one simply knows the numbers.
            AppError::PaymentRequired(_) | AppError::InsufficientCredits { .. } => {
                "insufficient_credits"
            }
            AppError::TooManyRequests(_) => "rate_limited",
            AppError::BadGateway(_) => "bad_gateway",
            AppError::Internal(_) => "internal_error",
        }
    }
}

impl AppError {
    /// The error as a client parses it: a stable `code` to branch on, the human `message`
    /// to show, and `data` when there is something actionable to hand over.
    ///
    /// One shape for both transports on purpose. A failure that happens before a chat
    /// stream opens arrives as this body in an HTTP response; the same failure mid-stream
    /// arrives as this body inside an SSE `error` event — and the client must be able to
    /// render the same thing from either, which it cannot do if one of them carries only
    /// a sentence.
    pub fn payload(&self) -> serde_json::Value {
        let mut body = json!({ "code": self.code(), "message": self.message() });

        if let AppError::InsufficientCredits {
            balance_tokens,
            required_tokens,
        } = self
        {
            body["data"] = json!({
                "balanceUsd": crate::models::tokens_to_usd(*balance_tokens),
                "tokens": balance_tokens,
                "requiredTokens": required_tokens,
                "requiredUsd": crate::models::tokens_to_usd(*required_tokens),
                "currency": crate::models::CREDIT_CURRENCY,
                "topUpPath": "/account",
                // The sentence the chat puts in the bubble, so the wording lives here and
                // not in three client surfaces.
                "message": "Your credit balance is too low for this message. Please recharge to continue.",
                "actionLabel": "Recharge",
            });
        }

        body
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let mut body = self.payload();
        body["status"] = json!("error");
        (status, Json(body)).into_response()
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
