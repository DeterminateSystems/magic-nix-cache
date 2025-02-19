{
  description = "GitHub Actions-powered Nix binary cache";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1.tar.gz";

    fenix = {
      url = "https://flakehub.com/f/nix-community/fenix/0.1.1727.tar.gz";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane = {
      url = "https://flakehub.com/f/ipetkov/crane/0.16.3.tar.gz";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nix.url = "https://flakehub.com/f/NixOS/nix/2.tar.gz";
  };

  outputs = { self, nixpkgs, fenix, crane, ... }@inputs:
    let
      supportedSystems = [
        "aarch64-linux"
        "x86_64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];

      forEachSupportedSystem = f: nixpkgs.lib.genAttrs supportedSystems (system: f rec {
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            self.overlays.default
          ];
        };
        inherit (pkgs) lib;
        inherit system;
      });

      fenixToolchain = system: with fenix.packages.${system};
        combine ([
          stable.clippy
          stable.rustc
          stable.cargo
          stable.rustfmt
          stable.rust-src
          stable.rust-analyzer
        ] ++ nixpkgs.lib.optionals (system == "x86_64-linux") [
          targets.x86_64-unknown-linux-musl.stable.rust-std
        ] ++ nixpkgs.lib.optionals (system == "aarch64-linux") [
          targets.aarch64-unknown-linux-musl.stable.rust-std
        ]);
    in
    {

      overlays.default = final: prev:
      let
          toolchain = fenixToolchain final.hostPlatform.system;
          craneLib = (crane.mkLib final).overrideToolchain toolchain;
          crateName = craneLib.crateNameFromCargoToml {
            cargoToml = ./magic-nix-cache/Cargo.toml;
          };

          commonArgs = {
            inherit (crateName) pname version;
            src = self;

            nativeBuildInputs = with final; [
              pkg-config
            ];

            buildInputs = [
              inputs.nix.packages.${final.stdenv.system}.default
              final.boost
            ] ++ final.lib.optionals final.stdenv.isDarwin [
              final.darwin.apple_sdk.frameworks.SystemConfiguration
              (final.libiconv.override { enableStatic = true; enableShared = false; })
            ];

            NIX_CFLAGS_LINK = final.lib.optionalString final.stdenv.isDarwin "-lc++abi";
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      in
      {
        magic-nix-cache = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
        });
      };

      packages = forEachSupportedSystem ({ pkgs, ... }: rec {
        magic-nix-cache = pkgs.magic-nix-cache;
        default = magic-nix-cache;

        veryLongChain =
          let
            ctx = ./README.md;

            # Function to write the current date to a file
            startFile =
              pkgs.stdenv.mkDerivation {
                name = "start-file";
                buildCommand = ''
                  cat ${ctx} > $out
                '';
              };

            # Recursive function to create a chain of derivations
            createChain = n: startFile:
              pkgs.stdenv.mkDerivation {
                name = "chain-${toString n}";
                src =
                  if n == 0 then
                    startFile
                  else createChain (n - 1) startFile;
                buildCommand = ''
                  echo $src  > $out
                '';
              };

          in
          # Starting point of the chain
          createChain 200 startFile;
      });

      devShells = forEachSupportedSystem ({ system, pkgs, lib }:
      let
          toolchain = fenixToolchain system;
      in
      {
        default = pkgs.mkShell {
          packages = with pkgs; [
            toolchain

            nix # for linking attic
            boost # for linking attic
            bashInteractive
            pkg-config

            cargo-bloat
            cargo-edit
            cargo-udeps
            cargo-watch
            bacon

            age
          ] ++ lib.optionals pkgs.stdenv.isDarwin [
            libiconv
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];

          NIX_CFLAGS_LINK = lib.optionalString pkgs.stdenv.isDarwin "-lc++abi";
          RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
        };
      });
    };
}
