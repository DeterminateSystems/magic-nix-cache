# gha-cache

`gha-cache` provides an async API to the GitHub Actions Cache API.
You can upload blobs with `AsyncRead` streams and obtain presigned URLs to download them.

## Introduction

The GitHub Actions Cache (hereinafter GHAC) service stores binary blobs [identified](https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows#matching-a-cache-key) by the following 3-tuple:

- **Cache Key**: The developer-specified name of the blob.
- **Cache Version**: A string identifying conditions that affect compatibility of the blob. It works like a namespace.
    - The official implementation uses a SHA256 hash of the paths and the compression method, but it can be anything.
    - In this crate, we let the user feed in arbitrary bytes to mutate the hash.
- **Cache Scope**: The branch containing the workflow run that uploaded the blob

### APIs

Two sets of APIs are in use:

- [GitHub Actions Cache API](https://github.com/actions/toolkit/blob/457303960f03375db6f033e214b9f90d79c3fe5c/packages/cache/src/internal/cacheHttpClient.ts#L38): Private API used by GHAC. This API allows uploading and downloading blobs.
    - Endpoint: `$ACTIONS_CACHE_URL`
    - Token: `$ACTIONS_RUNTIME_TOKEN`
- [GitHub REST API](https://docs.github.com/en/rest/actions/cache?apiVersion=2022-11-28#delete-github-actions-caches-for-a-repository-using-a-cache-key): Public API. This API allows listing and deleting blobs.
    - Endpoint: `$GITHUB_API_URL` / `https://api.github.com`
    - Token: `${{ secrets.GITHUB_TOKEN }}`

This crate supports only the former API.
We should contribute support for the latter to [Octocrab](https://github.com/XAMPPRocky/octocrab).

## Quick Start

Since GHAC uses private APIs that use special tokens for authentication, we need to get them from a workflow run.

The easiest way is with the `keygen` workflow in this repo.
Generate an `age` encryption key with `age-keygen -o key.txt`, and add the Public Key as a repository secret named `AGE_PUBLIC_KEY`.
Then, trigger the `keygen` workflow which will print out a command that will let you decrypt the credentials.
