# nix-actions-cache

`nix-actions-cache` is a minimal Nix Binary Cache server backed by [the GitHub Actions Cache](https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows).
It can be compiled into a ~3.5MB static binary for distribution, allowing it to start prefetching NARs used in a previous run even _before_ Nix is installed (not implemented yet).

## Development

This project depends on internal APIs used by the GitHub Actions Cache.
See `gha-cache/README.md` for more details on how to obtain the required tokens.

```
cargo run -- -c creds.json
cargo build --release --target x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
nix copy --to 'http://127.0.0.1:3000' $(which bash)
nix-store --store $PWD/test-root --extra-substituters 'http://localhost:3000' --option require-sigs false -r $(which bash)
```

## TODO

- [ ] Make a GitHub Action and dogfood
- [ ] Parallelize upload
- [ ] Make sure that the corresponding NAR exists before returning `.narinfo` request
- [ ] Keep in-memory cache of what's present
- [ ] Record what's accessed
- [ ] Prefetch previously-accessed NARs
