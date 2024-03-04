//! Errors.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("GitHub API error: {0}")]
    Api(#[from] gha_cache::api::Error),

    #[error("Not Found")]
    NotFound,

    #[error("Bad Request")]
    BadRequest,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to upload paths")]
    FailedToUpload,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let code = match &self {
            Self::Api(gha_cache::api::Error::ApiError {
                status: StatusCode::TOO_MANY_REQUESTS,
                ..
            }) => StatusCode::TOO_MANY_REQUESTS,
            // HACK: HTTP 418 makes Nix throw a visible error but not retry
            Self::Api(_) => StatusCode::IM_A_TEAPOT,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (code, format!("{}", self)).into_response()
    }
}
