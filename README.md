# Magic Nix Cache

**Magic Nix Cache** is a lightweight daemon that automatically pushes the outputs of every derivation that Nix builds in a given CI run to a configured [binary cache][binary-cache].
It can upload to [FlakeHub Cache][flakehub-cache], the [GitHub Actions cache][gha-cache], or both.

> [!IMPORTANT]
> **Are you a GitHub Actions user? Use [Magic Nix Cache Action][action].**
> It downloads, starts, and configures Magic Nix Cache for you, enabling you to save 30-50%+ of CI time without any effort or cost.
> Add it to your workflow after installing Nix:
>
> ```yaml
> - uses: DeterminateSystems/nix-installer-action@main
> - uses: DeterminateSystems/magic-nix-cache-action@main
> ```

## Ways to run Magic Nix Cache

| Method                                                                                   | Environment                                                                             | Where it stores builds                        |
| ---------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- | --------------------------------------------- |
| [FlakeHub Cache Action][flakehub-cache-action], which is based on Magic Nix Cache Action | GitHub Actions                                                                          | FlakeHub Cache                                |
| [Magic Nix Cache Action][action]                                                         | GitHub Actions                                                                          | GitHub Actions cache, FlakeHub Cache, or both |
| Run the daemon [directly](#running-the-daemon-directly)                                  | Other CI systems, such as [Buildkite][buildkite-guide] and [Semaphore][semaphore-guide] | FlakeHub Cache                                |

## Running the daemon directly

The GitHub Actions cache is only available inside GitHub Actions.
The daemon reads its credentials from the environment that GitHub Actions provides to every job (`ACTIONS_CACHE_URL`, `ACTIONS_RESULTS_URL`, `ACTIONS_RUNTIME_TOKEN`, and `ACTIONS_CACHE_SERVICE_V2`).
Nix can't fetch from the GitHub Actions cache directly, so the daemon also runs a small local server, set by `--listen`, that redirects Nix's requests to the GitHub Actions cache.

For FlakeHub Cache, the daemon needs FlakeHub credentials:

- If you use [Determinate Nix][determinate-nix], the daemon enables FlakeHub Cache automatically and uses Determinate Nix's credentials.
- Otherwise, pass a `netrc` file containing your FlakeHub token with `--flakehub-api-server-netrc`.

Run `magic-nix-cache --help` for the full list of options.
These are the ones you're most likely to need:

| Option                        | Description                                                                                                | Default          |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------- | ---------------- |
| `--listen`                    | The address to listen on.                                                                                  | `127.0.0.1:3000` |
| `--upstream`                  | An upstream cache. When the GitHub Actions cache doesn't have a store path, the daemon redirects Nix here. |                  |
| `--use-gha-cache`             | Whether to use the GitHub Actions cache.                                                                   |                  |
| `--use-flakehub`              | Whether to use FlakeHub Cache.                                                                             |                  |
| `--flakehub-api-server-netrc` | A `netrc` file containing your FlakeHub token. Ignored if Determinate Nix is available.                    |                  |
| `--nix-conf`                  | The `nix.conf` that the daemon writes its substituter and post-build hook settings to.                     |                  |
| `--cache-version`             | A version string for GitHub Actions cache entries. Changing it starts you with an empty cache.             |                  |
| `--diagnostic-endpoint`       | Where to send diagnostics. Set it to an empty string to disable [telemetry](#telemetry).                   |                  |

## Development

See [`DEVELOPMENT.md`](./DEVELOPMENT.md).

## Telemetry

The goal of Magic Nix Cache is to help teams save time in CI.
The daemon collects a little bit of telemetry information to help us make that true.

Here is a table of the [telemetry data we collect][telemetry]:

| Field                            | Use                                                                                                              |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `distinct_id`                    | An opaque string that represents your project, anonymized by sha256 hashing repository and organization details. |
| `version`                        | The version of Magic Nix Cache.                                                                                  |
| `is_ci`                          | Whether the Magic Nix Cache is running in any CI system, not only GitHub Actions.                                |
| `elapsed_seconds`                | How long the cache daemon was running.                                                                           |
| `narinfos_served`                | Number of narinfos served from the cache daemon.                                                                 |
| `narinfos_sent_upstream`         | Number of narinfo requests forwarded to the upstream cache.                                                      |
| `narinfos_negative_cache_hits`   | Effectiveness of an internal data structure which minimizes cache requests.                                      |
| `narinfos_negative_cache_misses` | Effectiveness of an internal data structure which minimizes cache requests.                                      |
| `narinfos_uploaded`              | Number of new narinfo files cached during this run.                                                              |
| `nars_served`                    | Number of nars served from the cache daemon.                                                                     |
| `nars_sent_upstream`             | Number of nar requests forwarded to the upstream cache.                                                          |
| `nars_uploaded`                  | Number of nars uploaded during this run.                                                                         |
| `num_original_paths`             | Number of store paths that existed on startup.                                                                   |
| `num_final_paths`                | Number of store paths that existed on shutdown.                                                                  |
| `num_new_paths`                  | The difference between `num_original_paths` and `num_final_paths`.                                               |

To disable diagnostic reporting, set the diagnostics URL to an empty string by passing `--diagnostic-endpoint=""`.
In [Magic Nix Cache Action][action], set the `diagnostic-endpoint` input to an empty string.

You can read the full [privacy policy][privacy] for [Determinate Systems][detsys], the creators of this tool and [Determinate Nix][determinate-nix].

## Acknowledgement

Magic Nix Cache is a collaboration with [Zhaofeng Li][zhaofeng].
Zhaofeng is a major contributor to the Nix community, having authored [Attic][attic] and [Colmena][colmena].
We'd like to express our deep gratitude to Zhaofeng for his tremendous work on this project.

[action]: https://github.com/DeterminateSystems/magic-nix-cache-action
[attic]: https://github.com/zhaofengli/attic
[buildkite-guide]: https://docs.determinate.systems/guides/buildkite
[binary-cache]: https://zero-to-nix.com/concepts/caching#binary-caches
[colmena]: https://github.com/zhaofengli/colmena
[determinate-nix]: https://docs.determinate.systems/determinate-nix
[detsys]: https://determinate.systems
[flakehub-cache]: https://flakehub.com/cache
[flakehub-cache-action]: https://github.com/DeterminateSystems/flakehub-cache-action
[gha-cache]: https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows
[privacy]: https://determinate.systems/policies/privacy
[semaphore-guide]: https://docs.determinate.systems/guides/semaphore
[telemetry]: https://github.com/DeterminateSystems/magic-nix-cache/blob/main/magic-nix-cache/src/telemetry.rs
[zhaofeng]: https://github.com/zhaofengli
