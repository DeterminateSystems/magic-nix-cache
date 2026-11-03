# Developing Magic Nix Cache

To work on GitHub Actions cache support locally, you need GitHub Actions cache credentials in your environment.
See [`gha-cache/README.md`](./gha-cache/README.md) for how that API works.

Run the daemon with an upstream cache:

```shell
cargo run -- --upstream https://cache.nixos.org
```

Build release binaries for Linux:

```shell
cargo build --release --target x86_64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

With the daemon running, push a store path to the cache:

```shell
nix copy --to 'http://127.0.0.1:3000' $(which bash)
```

Then fetch it back into a fresh test store:

```shell
nix-store --store $PWD/test-root --extra-substituters 'http://localhost:3000' --option require-sigs false -r $(which bash)
```
