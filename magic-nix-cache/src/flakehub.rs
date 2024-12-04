use crate::env::Environment;
use crate::error::{Error, Result};
use crate::DETERMINATE_NETRC_PATH;
use anyhow::Context;
use attic::cache::CacheName;
use attic::nix_store::{NixStore, StorePath};
use attic_client::push::{PushSession, PushSessionConfig};
use attic_client::{
    api::ApiClient,
    config::ServerConfig,
    push::{PushConfig, Pusher},
};

use reqwest::header::HeaderValue;
use reqwest::Url;
use serde::Deserialize;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;
use uuid::Uuid;

const USER_AGENT: &str = "magic-nix-cache";

pub struct State {
    #[allow(dead_code)]
    pub substituter: Url,

    pub push_session: PushSession,
}

pub async fn init_cache(
    environment: Environment,
    flakehub_api_server: &Url,
    flakehub_cache_server: &Url,
    flakehub_flake_name: &Option<String>,
    store: Arc<NixStore>,
    auth_method: &super::FlakeHubAuthSource,
) -> Result<State> {
    // Parse netrc to get the credentials for api.flakehub.com.
    let netrc_path = auth_method.as_path_buf();
    let NetrcInfo {
        netrc,
        flakehub_cache_server_hostname,
        flakehub_login,
        flakehub_password,
    } = extract_info_from_netrc(&netrc_path, flakehub_api_server, flakehub_cache_server).await?;

    if let super::FlakeHubAuthSource::Netrc(netrc_path) = auth_method {
        // Append an entry for the FlakeHub cache server to netrc.
        if !netrc
            .machines
            .iter()
            .any(|machine| machine.name.as_ref() == Some(&flakehub_cache_server_hostname))
        {
            let mut netrc_file = tokio::fs::OpenOptions::new()
                .create(false)
                .append(true)
                .open(netrc_path)
                .await
                .map_err(|e| {
                    Error::Internal(format!(
                        "Failed to open {} for appending: {}",
                        netrc_path.display(),
                        e
                    ))
                })?;

            netrc_file
                .write_all(
                    format!(
                        "\nmachine {} login {} password {}\n\n",
                        flakehub_cache_server_hostname, flakehub_login, flakehub_password,
                    )
                    .as_bytes(),
                )
                .await
                .map_err(|e| {
                    Error::Internal(format!(
                        "Failed to write credentials to {}: {}",
                        netrc_path.display(),
                        e
                    ))
                })?;
        }
    }

    let server_config = ServerConfig {
        endpoint: flakehub_cache_server.to_string(),
        token: Some(attic_client::config::ServerTokenConfig::Raw {
            token: flakehub_password.clone(),
        }),
    };
    let api_inner = ApiClient::from_server_config(server_config)?;
    let api = Arc::new(RwLock::new(api_inner));

    // Periodically refresh JWT in GitHub Actions environment
    if environment.is_github_actions() {
        match auth_method {
            super::FlakeHubAuthSource::Netrc(path) => {
                let netrc_path_clone = path.to_path_buf();
                let initial_github_jwt_clone = flakehub_password.clone();
                let flakehub_cache_server_clone = flakehub_cache_server.to_string();
                let api_clone = api.clone();

                tokio::task::spawn(refresh_github_actions_jwt_worker(
                    netrc_path_clone,
                    initial_github_jwt_clone,
                    flakehub_cache_server_clone,
                    api_clone,
                ));
            }
            crate::FlakeHubAuthSource::DeterminateNixd => {
                let api_clone = api.clone();
                let netrc_file = PathBuf::from(DETERMINATE_NETRC_PATH);
                let flakehub_api_server_clone = flakehub_api_server.clone();
                let flakehub_cache_server_clone = flakehub_cache_server.clone();

                let initial_meta = tokio::fs::metadata(&netrc_file).await.map_err(|e| {
                    Error::Io(e, format!("getting metadata of {}", netrc_file.display()))
                })?;
                let initial_inode = initial_meta.ino();

                tokio::task::spawn(refresh_determinate_token_worker(
                    netrc_file,
                    initial_inode,
                    flakehub_api_server_clone,
                    flakehub_cache_server_clone,
                    api_clone,
                ));
            }
        }
    }

    // Get the cache UUID for this project.
    let cache_name = {
        let mut url = flakehub_api_server
            .join("project")
            .map_err(|_| Error::Config(format!("bad URL '{}'", flakehub_api_server)))?;

        if let Some(flakehub_flake_name) = flakehub_flake_name {
            if !flakehub_flake_name.is_empty() {
                url = flakehub_api_server
                    .join(&format!("project/{}", flakehub_flake_name))
                    .map_err(|_| Error::Config(format!("bad URL '{}'", flakehub_api_server)))?;
            }
        }

        let response = reqwest::Client::new()
            .get(url.to_owned())
            .header("User-Agent", USER_AGENT)
            .basic_auth(flakehub_login, Some(&flakehub_password))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::GetCacheName(
                response.status(),
                response.text().await?,
            ));
        }

        #[derive(Deserialize)]
        struct ProjectInfo {
            organization_uuid_v7: Uuid,
            project_uuid_v7: Uuid,
        }

        let project_info = response.json::<ProjectInfo>().await?;

        format!(
            "{}:{}",
            project_info.organization_uuid_v7, project_info.project_uuid_v7,
        )
    };

    tracing::info!("Using cache {:?}", cache_name);

    let cache = unsafe { CacheName::new_unchecked(cache_name) };

    let cache_config = api.read().await.get_cache_config(&cache).await?;

    let push_config = PushConfig {
        num_workers: 5, // FIXME: use number of CPUs?
        force_preamble: false,
    };

    let mp = indicatif::MultiProgress::new();

    let push_session = Pusher::new(
        store.clone(),
        api.clone(),
        cache.to_owned(),
        cache_config,
        mp,
        push_config,
    )
    .into_push_session(PushSessionConfig {
        no_closure: false,
        ignore_upstream_cache_filter: false,
    });

    let state = State {
        substituter: flakehub_cache_server.to_owned(),
        push_session,
    };

    Ok(state)
}

#[derive(Debug)]
struct NetrcInfo {
    netrc: netrc_rs::Netrc,
    flakehub_cache_server_hostname: String,
    flakehub_login: String,
    flakehub_password: String,
}

#[tracing::instrument]
async fn extract_info_from_netrc(
    netrc_path: &Path,
    flakehub_api_server: &Url,
    flakehub_cache_server: &Url,
) -> Result<NetrcInfo> {
    let netrc = {
        let mut netrc_file = File::open(netrc_path).await.map_err(|e| {
            Error::Internal(format!("Failed to open {}: {}", netrc_path.display(), e))
        })?;
        let mut netrc_contents = String::new();
        netrc_file
            .read_to_string(&mut netrc_contents)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to read {} contents: {}",
                    netrc_path.display(),
                    e
                ))
            })?;
        netrc_rs::Netrc::parse(netrc_contents, false).map_err(Error::Netrc)?
    };

    let flakehub_netrc_entry = netrc
        .machines
        .iter()
        .find(|machine| {
            machine.name.as_ref() == flakehub_api_server.host().map(|x| x.to_string()).as_ref()
        })
        .ok_or_else(|| Error::MissingCreds(flakehub_api_server.to_string()))?
        .to_owned();

    let flakehub_cache_server_hostname = flakehub_cache_server
        .host()
        .ok_or_else(|| Error::BadUrl(flakehub_cache_server.to_owned()))?
        .to_string();
    let flakehub_login = flakehub_netrc_entry.login.ok_or_else(|| {
        Error::Config(format!(
            "netrc file does not contain a login for '{}'",
            flakehub_api_server
        ))
    })?;
    let flakehub_password = flakehub_netrc_entry.password.ok_or_else(|| {
        Error::Config(format!(
            "netrc file does not contain a password for '{}'",
            flakehub_api_server
        ))
    })?;

    Ok(NetrcInfo {
        netrc,
        flakehub_cache_server_hostname,
        flakehub_login,
        flakehub_password,
    })
}

pub async fn enqueue_paths(state: &State, store_paths: Vec<StorePath>) -> Result<()> {
    state.push_session.queue_many(store_paths)?;

    Ok(())
}

/// Refresh the GitHub Actions JWT every 2 minutes (slightly less than half of the default validity
/// period) to ensure pushing / pulling doesn't stop working.
#[tracing::instrument(skip_all)]
async fn refresh_github_actions_jwt_worker(
    netrc_path: std::path::PathBuf,
    mut github_jwt: String,
    flakehub_cache_server_clone: String,
    api: Arc<RwLock<ApiClient>>,
) -> Result<()> {
    // NOTE(cole-h): This is a workaround -- at the time of writing, GitHub Actions JWTs are only
    // valid for 5 minutes after being issued. FlakeHub uses these JWTs for authentication, which
    // means that after those 5 minutes have passed and the token is expired, FlakeHub (and by
    // extension FlakeHub Cache) will no longer allow requests using this token. However, GitHub
    // gives us a way to repeatedly request new tokens, so we utilize that and refresh the token
    // every 2 minutes (less than half of the lifetime of the token).

    // TODO(cole-h): this should probably be half of the token's lifetime ((exp - iat) / 2), but
    // getting this is nontrivial so I'm not going to do it until GitHub changes the lifetime and
    // breaks this.
    let next_refresh = std::time::Duration::from_secs(2 * 60);

    // NOTE(cole-h): we sleep until the next refresh at first because we already got a token from
    // GitHub recently, don't need to try again until we actually might need to get a new one.
    tokio::time::sleep(next_refresh).await;

    // NOTE(cole-h): https://docs.github.com/en/actions/deployment/security-hardening-your-deployments/configuring-openid-connect-in-cloud-providers#requesting-the-jwt-using-environment-variables
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        HeaderValue::from_static("application/json;api-version=2.0"),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );

    let github_client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .build()?;

    loop {
        match rewrite_github_actions_token(&github_client, &netrc_path, &github_jwt).await {
            Ok(new_github_jwt) => {
                github_jwt = new_github_jwt;

                let server_config = ServerConfig {
                    endpoint: flakehub_cache_server_clone.clone(),
                    token: Some(attic_client::config::ServerTokenConfig::Raw {
                        token: github_jwt.clone(),
                    }),
                };
                let new_api = ApiClient::from_server_config(server_config)?;

                {
                    let mut api_client = api.write().await;
                    *api_client = new_api;
                }

                tracing::debug!(
                    "Stored new token in netrc and API client, sleeping for {next_refresh:?}"
                );
                tokio::time::sleep(next_refresh).await;
            }
            Err(e) => {
                tracing::error!(
                    ?e,
                    "Failed to get a new JWT from GitHub, trying again in 10 seconds"
                );
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        }
    }
}

#[tracing::instrument(skip_all)]
async fn rewrite_github_actions_token(
    client: &reqwest::Client,
    netrc_path: &Path,
    old_github_jwt: &str,
) -> Result<String> {
    // NOTE(cole-h): https://docs.github.com/en/actions/deployment/security-hardening-your-deployments/configuring-openid-connect-in-cloud-providers#requesting-the-jwt-using-environment-variables
    let runtime_token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").map_err(|e| {
        Error::Internal(format!(
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN was invalid unicode: {e}"
        ))
    })?;
    let runtime_url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").map_err(|e| {
        Error::Internal(format!(
            "ACTIONS_ID_TOKEN_REQUEST_URL was invalid unicode: {e}"
        ))
    })?;

    let token_request_url = format!("{runtime_url}&audience=api.flakehub.com");
    let token_response = client
        .request(reqwest::Method::GET, &token_request_url)
        .bearer_auth(runtime_token)
        .send()
        .await
        .with_context(|| format!("sending request to {token_request_url}"))?;

    if let Err(e) = token_response.error_for_status_ref() {
        tracing::error!(?e, "Got error response when requesting token");
        return Err(e)?;
    }

    #[derive(serde::Deserialize)]
    struct TokenResponse {
        value: String,
    }

    let token_response: TokenResponse = token_response
        .json()
        .await
        .with_context(|| "converting response into json")?;

    let new_github_jwt_string = token_response.value;
    let netrc_contents = tokio::fs::read_to_string(netrc_path)
        .await
        .with_context(|| format!("failed to read {netrc_path:?} to string"))?;
    let new_netrc_contents = netrc_contents.replace(old_github_jwt, &new_github_jwt_string);

    // NOTE(cole-h): create the temporary file right next to the real one so we don't run into
    // cross-device linking issues when renaming
    let netrc_path_tmp = netrc_path.with_extension("tmp");
    tokio::fs::write(&netrc_path_tmp, new_netrc_contents)
        .await
        .with_context(|| format!("writing new JWT to {netrc_path_tmp:?}"))?;
    tokio::fs::rename(&netrc_path_tmp, &netrc_path)
        .await
        .with_context(|| format!("renaming {netrc_path_tmp:?} to {netrc_path:?}"))?;

    Ok(new_github_jwt_string)
}

#[tracing::instrument(skip_all)]
async fn refresh_determinate_token_worker(
    netrc_file: PathBuf,
    mut inode: u64,
    flakehub_api_server: Url,
    flakehub_cache_server: Url,
    api_clone: Arc<RwLock<ApiClient>>,
) {
    // NOTE(cole-h): This is a workaround -- at the time of writing, determinate-nixd handles the
    // GitHub Actions JWT refreshing for us, which means we don't know when this will happen. At the
    // moment, it does it roughly every 2 minutes (less than half of the total lifetime of the
    // issued token), so refreshing every 30 seconds is "fine".

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        let meta = tokio::fs::metadata(&netrc_file)
            .await
            .map_err(|e| Error::Io(e, format!("getting metadata of {}", netrc_file.display())));

        let Ok(meta) = meta else {
            tracing::error!(e = ?meta);
            continue;
        };

        let current_inode = meta.ino();

        if current_inode == inode {
            tracing::debug!("current inode is the same, file didn't change");
            continue;
        }

        tracing::debug!("current inode is different, file changed");
        inode = current_inode;

        let flakehub_password = match extract_info_from_netrc(
            &netrc_file,
            &flakehub_api_server,
            &flakehub_cache_server,
        )
        .await
        {
            Ok(NetrcInfo {
                flakehub_password, ..
            }) => flakehub_password,
            Err(e) => {
                tracing::error!(?e, "Failed to extract auth info from netrc");
                continue;
            }
        };

        let server_config = ServerConfig {
            endpoint: flakehub_cache_server.to_string(),
            token: Some(attic_client::config::ServerTokenConfig::Raw {
                token: flakehub_password,
            }),
        };

        let new_api = ApiClient::from_server_config(server_config.clone());

        let Ok(new_api) = new_api else {
            tracing::error!(e = ?new_api, "Failed to construct new ApiClient");
            continue;
        };

        {
            let mut api_client = api_clone.write().await;
            *api_client = new_api;
        }

        tracing::debug!("Stored new token in API client, sleeping for 30s");
    }
}
