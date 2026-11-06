//! What a handler answers when something is wrong. The pages read the `error` text of the JSON and show it.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

pub enum AppError {
    /// The request itself is wrong (400): the text is shown to the person.
    Bad(String),
    /// Not signed in (401).
    Unauthorized,
    /// Wrong sign-in details (401) with a message to show.
    Denied(String),
    /// Signed in but not allowed (403).
    Forbidden(String),
    /// Too many attempts (429).
    TooMany(String),
    NotFound,
    /// A bug or the database is down (500): logged, and shown only in general terms.
    Internal(anyhow::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn bad(text: impl Into<String>) -> Self {
        Self::Bad(text.into())
    }
    #[allow(non_snake_case)]
    pub fn Unauthorized_with(text: impl Into<String>) -> Self {
        Self::Denied(text.into())
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self::Internal(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, text) = match self {
            Self::Bad(t) => (StatusCode::BAD_REQUEST, t),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Please sign in.".to_string()),
            Self::Denied(t) => (StatusCode::UNAUTHORIZED, t),
            Self::Forbidden(t) => (StatusCode::FORBIDDEN, t),
            Self::TooMany(t) => (StatusCode::TOO_MANY_REQUESTS, t),
            Self::NotFound => (StatusCode::NOT_FOUND, "Not found".to_string()),
            Self::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the server.".to_string())
            }
        };
        (status, [("Cache-Control", "no-store")], Json(json!({ "error": text }))).into_response()
    }
}
