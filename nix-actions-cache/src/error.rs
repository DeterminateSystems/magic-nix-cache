//! Errors.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use thiserror::Error;

use gha_cache::api::Error as ApiError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("GitHub API error")]
    ApiError(#[from] ApiError),

    #[error("Not Found")]
    NotFound,

    #[error("Bad Request")]
    BadRequest,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let code = match &self {
            Self::ApiError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest => StatusCode::BAD_REQUEST,
        };

        (code, format!("{}", self)).into_response()
    }
}
