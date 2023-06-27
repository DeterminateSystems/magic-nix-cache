//! Action API.
//!
//! This API is intended to be used by nix-installer-action.

use std::net::SocketAddr;

use axum::{extract::Extension, http::uri::Uri, routing::post, Json, Router};
use axum_macros::debug_handler;
use serde::Serialize;

use super::State;
use crate::error::Result;
use crate::util::{get_store_paths, upload_paths};

#[derive(Debug, Clone, Serialize)]
struct WorkflowStartResponse {
    num_original_paths: usize,
}

// bogus change

#[derive(Debug, Clone, Serialize)]
struct WorkflowFinishResponse {
    num_original_paths: usize,
    num_final_paths: usize,
    num_new_paths: usize,
}

pub fn get_router() -> Router {
    Router::new()
        .route("/api/workflow-start", post(workflow_start))
        .route("/api/workflow-finish", post(workflow_finish))
}

/// Record existing paths.
#[debug_handler]
async fn workflow_start(Extension(state): Extension<State>) -> Result<Json<WorkflowStartResponse>> {
    tracing::info!("Workflow started");

    let mut original_paths = state.original_paths.lock().await;
    *original_paths = get_store_paths().await?;

    Ok(Json(WorkflowStartResponse {
        num_original_paths: original_paths.len(),
    }))
}

/// Push new paths and shut down.
async fn workflow_finish(
    Extension(state): Extension<State>,
) -> Result<Json<WorkflowFinishResponse>> {
    tracing::info!("Workflow finished");

    let original_paths = state.original_paths.lock().await;
    let final_paths = get_store_paths().await?;
    let new_paths = final_paths
        .difference(&original_paths)
        .cloned()
        .collect::<Vec<_>>();

    tracing::info!("Pushing {} new paths", new_paths.len());
    let store_uri = make_store_uri(&state.self_endpoint);
    upload_paths(new_paths.clone(), &store_uri).await?;

    let sender = state.shutdown_sender.lock().await.take().unwrap();
    sender.send(()).unwrap();

    let reply = WorkflowFinishResponse {
        num_original_paths: original_paths.len(),
        num_final_paths: final_paths.len(),
        num_new_paths: new_paths.len(),
    };

    state
        .metrics
        .num_original_paths
        .set(reply.num_original_paths);
    state.metrics.num_final_paths.set(reply.num_final_paths);
    state.metrics.num_new_paths.set(reply.num_new_paths);

    Ok(Json(reply))
}

fn make_store_uri(self_endpoint: &SocketAddr) -> String {
    Uri::builder()
        .scheme("http")
        .authority(self_endpoint.to_string())
        .path_and_query("/?compression=zstd&parallel-compression=true")
        .build()
        .unwrap()
        .to_string()
}
