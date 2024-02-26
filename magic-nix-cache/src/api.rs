//! Action API.
//!
//! This API is intended to be used by nix-installer-action.

use std::net::SocketAddr;

use axum::{extract::Extension, http::uri::Uri, routing::post, Json, Router};
use axum_macros::debug_handler;
use serde::{Deserialize, Serialize};

use super::State;
use crate::error::{Error, Result};
use crate::util::{get_store_paths, upload_paths};

#[derive(Debug, Clone, Serialize)]
struct WorkflowStartResponse {
    num_original_paths: usize,
}

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
        .route("/api/enqueue-paths", post(enqueue_paths))
}

/// Record existing paths.
#[debug_handler]
async fn workflow_start(Extension(state): Extension<State>) -> Result<Json<WorkflowStartResponse>> {
    tracing::info!("Workflow started");

    let mut original_paths = state.original_paths.lock().await;
    *original_paths = get_store_paths(&state.store).await?;

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
    let final_paths = get_store_paths(&state.store).await?;
    let new_paths = final_paths
        .difference(&original_paths)
        .cloned()
        .collect::<Vec<_>>();

    if state.api.is_some() {
        tracing::info!("Pushing {} new paths to GHA cache", new_paths.len());
        let store_uri = make_store_uri(&state.self_endpoint);
        upload_paths(new_paths.clone(), &store_uri).await?;
    }

    if let Some(sender) = state.shutdown_sender.lock().await.take() {
        sender
            .send(())
            .map_err(|_| Error::Internal("Sending shutdown server message".to_owned()))?;

        // Wait for the Attic push workers to finish.
        if let Some(attic_state) = state.flakehub_state.write().await.take() {
            attic_state.push_session.wait().await?;
        }
    }

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
        .expect("Cannot construct URL to self")
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueuePathsRequest {
    pub store_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueuePathsResponse {}

/// Schedule paths in the local Nix store for uploading.
async fn enqueue_paths(
    Extension(state): Extension<State>,
    Json(req): Json<EnqueuePathsRequest>,
) -> Result<Json<EnqueuePathsResponse>> {
    tracing::info!("Enqueueing {:?}", req.store_paths);

    let store_paths = req
        .store_paths
        .iter()
        .map(|path| state.store.follow_store_path(path).map_err(Error::Attic))
        .collect::<Result<Vec<_>>>()?;

    if let Some(flakehub_state) = &*state.flakehub_state.read().await {
        crate::flakehub::enqueue_paths(flakehub_state, store_paths).await?;
    }

    Ok(Json(EnqueuePathsResponse {}))
}
