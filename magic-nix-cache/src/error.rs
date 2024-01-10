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

    #[error("GHA cache is disabled")]
    GHADisabled,

    #[error("FlakeHub cache error: {0}")]
    FlakeHub(#[from] anyhow::Error),

    #[error("FlakeHub HTTP error: {0}")]
    FlakeHubHttp(#[from] reqwest::Error),

    #[error("Got HTTP response {1} getting FlakeHub binary cache creation token from {0}: {2}")]
    CacheCreation(reqwest::Url, reqwest::StatusCode, String),

    #[error("netrc parse error: {0}")]
    Netrc(netrc_rs::Error),

    #[error("Cannot find netrc credentials for {0}")]
    MissingCreds(String),

    #[error("Received bad JWT")]
    BadJWT,

    #[error("Received bad JWT token: {0}")]
    JWTParsing(#[from] jwt::Error),

    #[error("Attic error: {0}")]
    Attic(#[from] attic::AtticError),

    #[error("Bad URL")]
    BadURL(reqwest::Url),

    #[error("FlakeHub did not return any cache for the calling user")]
    NoKnownCaches,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let code = match &self {
            // HACK: HTTP 418 makes Nix throw a visible error but not retry
            Self::Api(_) => StatusCode::IM_A_TEAPOT,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (code, format!("{}", self)).into_response()
    }
}
