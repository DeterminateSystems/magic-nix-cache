use crate::error::{Error, Result};
use attic::api::v1::cache_config::{CreateCacheRequest, KeypairConfig};
use attic::cache::CacheSliceIdentifier;
use attic::nix_store::{NixStore, StorePath};
use attic_client::push::{PushSession, PushSessionConfig};
use attic_client::{
    api::{ApiClient, ApiError},
    config::ServerConfig,
    push::{PushConfig, Pusher},
};
use reqwest::Url;
use serde::Deserialize;
use std::env;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

const JWT_PREFIX: &str = "flakehub1_";
const USER_AGENT: &str = "magic-nix-cache";

pub struct State {
    pub substituter: Url,

    pub push_session: PushSession,
}

pub async fn init_cache(
    flakehub_api_server: &Url,
    flakehub_api_server_netrc: &Path,
    flakehub_cache_server: &Url,
    store: Arc<NixStore>,
) -> Result<State> {
    // Parse netrc to get the credentials for api.flakehub.com.
    let netrc = {
        let mut netrc_file = File::open(flakehub_api_server_netrc).await?;
        let mut netrc_contents = String::new();
        netrc_file.read_to_string(&mut netrc_contents).await?;
        netrc_rs::Netrc::parse(netrc_contents, false).map_err(Error::Netrc)?
    };

    let netrc_entry = {
        netrc
            .machines
            .iter()
            .find(|machine| {
                machine.name.as_ref() == flakehub_api_server.host().map(|x| x.to_string()).as_ref()
            })
            .ok_or_else(|| Error::MissingCreds(flakehub_api_server.to_string()))?
            .to_owned()
    };

    let flakehub_cache_server_hostname = flakehub_cache_server
        .host()
        .ok_or_else(|| Error::BadURL(flakehub_cache_server.to_owned()))?
        .to_string();

    // Append an entry for the FlakeHub cache server to netrc.
    if !netrc
        .machines
        .iter()
        .any(|machine| machine.name.as_ref() == Some(&flakehub_cache_server_hostname))
    {
        let mut netrc_file = tokio::fs::OpenOptions::new()
            .create(false)
            .append(true)
            .open(flakehub_api_server_netrc)
            .await?;
        netrc_file
            .write_all(
                format!(
                    "\nmachine {} password {}\n\n",
                    flakehub_cache_server_hostname,
                    netrc_entry.password.as_ref().unwrap(),
                )
                .as_bytes(),
            )
            .await?;
    }

    // Get the cache we're supposed to use.
    let expected_cache_name = {
        let github_repo = env::var("GITHUB_REPOSITORY")
            .expect("GITHUB_REPOSITORY environment variable is not set");

        let url = flakehub_api_server
            .join(&format!("project/{}", github_repo))
            .unwrap();

        let response = reqwest::Client::new()
            .get(url.to_owned())
            .header("User-Agent", USER_AGENT)
            .basic_auth(
                netrc_entry.login.as_ref().unwrap(),
                netrc_entry.password.as_ref(),
            )
            .send()
            .await?;

        if response.status().is_success() {
            #[derive(Deserialize)]
            struct ProjectInfo {
                organization_uuid_v7: Uuid,
                project_uuid_v7: Uuid,
            }

            let project_info = response.json::<ProjectInfo>().await?;

            let expected_cache_name = format!(
                "{}:{}",
                project_info.organization_uuid_v7, project_info.project_uuid_v7,
            );

            tracing::info!("Want to use cache {:?}.", expected_cache_name);

            Some(expected_cache_name)
        } else {
            tracing::error!(
                "Failed to get project info from {}: {}",
                url,
                response.status()
            );
            None
        }
    };

    // Get a token for creating and pushing to the FlakeHub binary cache.
    let (known_caches, token) = {
        let url = flakehub_api_server.join("cache/token").unwrap();

        let request = reqwest::Client::new()
            .post(url.to_owned())
            .header("User-Agent", USER_AGENT)
            .basic_auth(
                netrc_entry.login.as_ref().unwrap(),
                netrc_entry.password.as_ref(),
            );

        let response = request.send().await?;

        if !response.status().is_success() {
            return Err(Error::CacheCreation(
                url,
                response.status(),
                response.text().await?,
            ));
        }

        #[derive(Deserialize)]
        struct Response {
            token: String,
        }

        let token = response.json::<Response>().await?.token;

        // Parse the JWT to get the list of caches to which we have access.
        let jwt = token.strip_prefix(JWT_PREFIX).ok_or(Error::BadJWT)?;
        let jwt_parsed: jwt::Token<jwt::Header, serde_json::Map<String, serde_json::Value>, _> =
            jwt::Token::parse_unverified(jwt)?;
        let known_caches = jwt_parsed
            .claims()
            .get("https://cache.flakehub.com/v1")
            .ok_or(Error::BadJWT)?
            .get("caches")
            .ok_or(Error::BadJWT)?
            .as_object()
            .ok_or(Error::BadJWT)?;

        (known_caches.to_owned(), token)
    };

    // Use the expected cache if we have access to it, otherwise use
    // the oldest cache to which we have access.
    let cache_name = {
        if expected_cache_name
            .as_ref()
            .map_or(false, |x| known_caches.get(x).is_some())
        {
            expected_cache_name.unwrap().to_owned()
        } else {
            let mut keys: Vec<_> = known_caches.keys().collect();
            keys.sort();
            keys.first()
                .expect("FlakeHub did not return any cache for the calling user.")
                .to_string()
        }
    };

    let cache = CacheSliceIdentifier::from_str(&cache_name)?;

    tracing::info!("Using cache {}.", cache);

    // Create the cache.
    let api = ApiClient::from_server_config(ServerConfig {
        endpoint: flakehub_cache_server.to_string(),
        //token: netrc_entry.password.as_ref().cloned(),
        token: Some(token.to_owned()),
    })?;

    let request = CreateCacheRequest {
        keypair: KeypairConfig::Generate,
        is_public: false,
        priority: 39,
        store_dir: "/nix/store".to_owned(),
        upstream_cache_key_names: vec!["cache.nixos.org-1".to_owned()], // FIXME: do we want this?
    };

    if let Err(err) = api.create_cache(&cache, request).await {
        match err.downcast_ref::<ApiError>() {
            Some(ApiError::Structured(x)) if x.error == "CacheAlreadyExists" => {
                tracing::info!("Cache {} already exists.", cache_name);
            }
            _ => {
                return Err(Error::FlakeHub(err));
            }
        }
    } else {
        tracing::info!("Created cache {} on {}.", cache_name, flakehub_cache_server);
    }

    let cache_config = api.get_cache_config(&cache).await?;

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

    Ok(State {
        substituter: flakehub_cache_server.to_owned(),
        push_session,
    })
}

pub async fn enqueue_paths(state: &State, store_paths: Vec<StorePath>) -> Result<()> {
    state.push_session.queue_many(store_paths)?;

    Ok(())
}
