//! Errors.

use std::io::Error as IoError;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use thiserror::Error;

use gha_cache::api::Error as ApiError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("GitHub API error: {0}")]
    ApiError(#[from] ApiError),

    #[error("Not Found")]
    NotFound,

    #[error("Bad Request")]
    BadRequest,

    #[error("I/O error: {0}")]
    IoError(#[from] IoError),

    #[error("Failed to upload paths")]
    FailedToUpload,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let code = match &self {
            // HACK: HTTP 418 makes Nix throw a visible error but not retry
            Self::ApiError(_) => StatusCode::IM_A_TEAPOT,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (code, format!("{}", self)).into_response()
    }
}
