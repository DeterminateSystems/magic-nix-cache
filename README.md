# Magic Nix Cache

Save 30-50%+ of CI time without any effort or cost.
Use Magic Nix Cache, a totally free and zero-configuration binary cache for Nix on GitHub Actions.

Add our [GitHub Action][action] after installing Nix, in your workflow, like this:

```yaml
      - uses: DeterminateSystems/magic-nix-cache-action@main
```

See [Usage](#usage) for a detailed example.

## Why use the Magic Nix Cache?
Magic Nix Cache uses the GitHub Actions [built-in cache][ghacache] to share builds between Workflow runs, and has many advantages over alternatives.

1. Totally free: backed by GitHub Actions' cache, there is no additional service to pay for.
1. Zero configuration: add our action to your workflow.
   That's it.
   Everything built in your workflow will be cached.
1. No secrets: Forks and pull requests benefit from the cache, too.
1. Secure: Magic Nix Cache follows the [same semantics as the GitHub Actions cache][semantics], and malicious pull requests cannot pollute your project.
1. Private: The cache is stored in the GitHub Actions cache, not with an additional third party.

> **Note:** the Magic Nix Cache doesn't offer a publically available cache.
> This means the cache is only usable in CI.
> Zero to Nix has an article on binary caching if you want to [share Nix builds][z2ncache] with users outside of CI.

## Usage

Add it to your Linux and macOS GitHub Actions workflows, like this:

```yaml
name: CI

on:
  push:
  pull_request:

jobs:
  check:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v3
      - uses: DeterminateSystems/nix-installer-action@main
      - uses: DeterminateSystems/magic-nix-cache-action@main
      - run: nix flake check
```

That's it.
Everything built in your workflow will be cached.

## Usage Notes

The GitHub Actions Cache has a rate limit on reads and writes.
Occasionally, large projects or large rebuilds may exceed those rate-limits, and you'll see evidence of that in your logs.
The error looks like this:

```
error: unable to download 'http://127.0.0.1:37515/<...>': HTTP error 418
       response body:
       GitHub API error: API error (429 Too Many Requests): StructuredApiError { message: "Request was blocked due to exceeding usage of resource 'Count' in namespace ''." }
```

The caching daemon and Nix both handle this gracefully, and won't not cause your CI to fail.
When the rate limit is exceeded while pulling dependencies, your workflow may perform more builds than usual.
When the rate limit is exceeded while uploading to the cache, the remainder of those store paths will be uploaded on the next run of the workflow.

## Development

This project depends on the GitHub Actions Cache API.
For local development, see `gha-cache/README.md` for more details on how to obtain the required tokens.

```
cargo run -- -c creds.json --upstream https://cache.nixos.org
cargo build --release --target x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
nix copy --to 'http://127.0.0.1:3000' $(which bash)
nix-store --store $PWD/test-root --extra-substituters 'http://localhost:3000' --option require-sigs false -r $(which bash)
```

## Acknowledgement
Magic Nix Cache is a collaboration with [Zhaofeng Li][zhaofeng].
Zhaofeng is a major contributor to the Nix community, having authored [Attic][attic] and [Colmena][colmena].
We'd like to express our deep gratitude to Zhaofeng for his tremendous work on this project.

## Telemetry

The goal of Magic Nix Cache is to help teams save time in CI.
The cache daemon collects a little bit of telemetry information to help us make that true.

Here is a table of the [telemetry data we collect][telemetry]:

| Field                            | Use                                                                                                              |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `distinct_id`                    | An opaque string that represents your project, anonymized by sha256 hashing repository and organization details. |
| `version`                        | The version of Magic Nix Cache.                                                                                  |
| `is_ci`                          | Whether the Magic Nix Cache is being used in CI (i.e.: GitHub Actions).                                          |
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

You can read the full privacy policy for [Determinate Systems][detsys], the creators of this tool and the [Determinate Nix Installer][installer], [here][privacy].

[detsys]: https://determinate.systems/
[action]: https://github.com/DeterminateSystems/magic-nix-cache-action/
[installer]: https://github.com/DeterminateSystems/nix-installer/
[ghacache]: https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows
[privacy]: https://determinate.systems/privacy
[telemetry]: https://github.com/DeterminateSystems/magic-nix-cache/blob/main/magic-nix-cache/src/telemetry.rs
[semantics]: https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows#restrictions-for-accessing-a-cache
[z2ncache]: https://zero-to-nix.com/concepts/caching#binary-caches
[zhaofeng]: https://github.com/zhaofengli/
[attic]: https://github.com/zhaofengli/attic
[colmena]: https://github.com/zhaofengli/colmena
