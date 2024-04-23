//! Action API.
//!
//! This API is intended to be used by nix-installer-action.

use axum::{extract::Extension, routing::post, Json, Router};
use axum_macros::debug_handler;
use serde::{Deserialize, Serialize};

use super::State;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize)]
struct WorkflowStartResponse {}

#[derive(Debug, Clone, Serialize)]
struct WorkflowFinishResponse {
    //num_new_paths: usize,
}

pub fn get_router() -> Router {
    Router::new()
        .route("/api/workflow-start", post(workflow_start))
        .route("/api/workflow-finish", post(workflow_finish))
        .route("/api/enqueue-paths", post(enqueue_paths))
}

/// Record existing paths.
#[debug_handler]
async fn workflow_start(
    Extension(_state): Extension<State>,
) -> Result<Json<WorkflowStartResponse>> {
    tracing::info!("Workflow started");

    Ok(Json(WorkflowStartResponse {}))
}

/// Push new paths and shut down.
async fn workflow_finish(
    Extension(state): Extension<State>,
) -> Result<Json<WorkflowFinishResponse>> {
    tracing::info!("Workflow finished");

    if let Some(gha_cache) = &state.gha_cache {
        tracing::info!("Waiting for GitHub action cache uploads to finish");
        gha_cache.shutdown().await?;
    }

    if let Some(sender) = state.shutdown_sender.lock().await.take() {
        sender
            .send(())
            .map_err(|_| Error::Internal("Sending shutdown server message".to_owned()))?;

        // Wait for the Attic push workers to finish.
        if let Some(attic_state) = state.flakehub_state.write().await.take() {
            tracing::info!("Waiting for FlakeHub cache uploads to finish");
            attic_state.push_session.wait().await?;
        }
    }

    // NOTE(cole-h): see `init_logging`
    let logfile = std::env::temp_dir().join("magic-nix-cache-tracing.log");
    let logfile_contents = std::fs::read_to_string(logfile)?;
    println!("Every log line throughout the lifetime of the program:");
    println!("\n{logfile_contents}\n");

    let reply = WorkflowFinishResponse {};

    //state.metrics.num_new_paths.set(num_new_paths);

    Ok(Json(reply))
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

    if let Some(gha_cache) = &state.gha_cache {
        gha_cache
            .enqueue_paths(state.store.clone(), store_paths.clone())
            .await?;
    }

    if let Some(flakehub_state) = &*state.flakehub_state.read().await {
        crate::flakehub::enqueue_paths(flakehub_state, store_paths).await?;
    }

    Ok(Json(EnqueuePathsResponse {}))
}
