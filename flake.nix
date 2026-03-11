{
  description = "GitHub Actions-powered Nix binary cache";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";

    crane.url = "https://flakehub.com/f/ipetkov/crane/*";

    #nix.url = "https://flakehub.com/f/DeterminateSystems/nix-src/=3.16.*";
    nix.url = "github:DeterminateSystems/nix-src/provenance-tags";

  };

  outputs =
    inputs:
    let
      supportedSystems = [
        "aarch64-linux"
        "x86_64-linux"
        "aarch64-darwin"
      ];

      forEachSupportedSystem =
        f:
        inputs.nixpkgs.lib.genAttrs supportedSystems (
          system:
          f rec {
            pkgs = import inputs.nixpkgs {
              inherit system;
              overlays = [
              ];
            };
            inherit system;
          }
        );
    in
    {

      packages = forEachSupportedSystem (
        { pkgs, system, ... }:
        let
          pkgs' = pkgs.pkgsStatic;
          craneLib = inputs.crane.mkLib pkgs';
          crateName = craneLib.crateNameFromCargoToml {
            cargoToml = ./magic-nix-cache/Cargo.toml;
          };

          rustTargetSpec = pkgs'.stdenv.hostPlatform.rust.rustcTargetSpec;
          rustTargetSpecEnv = pkgs'.lib.toUpper (builtins.replaceStrings [ "-" ] [ "_" ] rustTargetSpec);

          commonArgs = {
            inherit (crateName) pname version;
            src = inputs.self;

            depsBuildBuild = with pkgs'; [
              buildPackages.stdenv.cc
              lld
            ];

            nativeBuildInputs = with pkgs'; [
              pkg-config
              protobuf
            ];

            buildInputs = [
              inputs.nix.packages.${system}.nix-util-static
              inputs.nix.packages.${system}.nix-store-static
              inputs.nix.packages.${system}.nix-main-static
              inputs.nix.packages.${system}.nix-expr-static
              pkgs'.boost
            ];

            doIncludeCrossToolchainEnv = false;

            env.CARGO_BUILD_TARGET = rustTargetSpec;
            env."CARGO_TARGET_${rustTargetSpecEnv}_LINKER" = "${pkgs'.stdenv.cc.targetPrefix}cc";
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        rec {
          magic-nix-cache = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
            }
          );

          default = magic-nix-cache;

          veryLongChain =
            let
              ctx = ./README.md;

              # Function to write the current date to a file
              startFile = pkgs.stdenv.mkDerivation {
                name = "start-file";
                buildCommand = ''
                  cat ${ctx} > $out
                '';
              };

              # Recursive function to create a chain of derivations
              createChain =
                n: startFile:
                pkgs.stdenv.mkDerivation {
                  name = "chain-${toString n}";
                  src = if n == 0 then startFile else createChain (n - 1) startFile;
                  buildCommand = ''
                    echo $src  > $out
                  '';
                };

            in
            # Starting point of the chain
            createChain 200 startFile;
        }
      );

      devShells = forEachSupportedSystem (
        { system, pkgs }:
        let
          pkgs' = pkgs.pkgsStatic;
          rustTargetSpec = pkgs'.stdenv.hostPlatform.rust.rustcTargetSpec;
          rustTargetSpecEnv = pkgs'.lib.toUpper (builtins.replaceStrings [ "-" ] [ "_" ] rustTargetSpec);
        in
        {
          default = pkgs'.mkShell {
            env.CARGO_BUILD_TARGET = rustTargetSpec;
            env."CARGO_TARGET_${rustTargetSpecEnv}_LINKER" = "${pkgs'.stdenv.cc.targetPrefix}cc";
            env.RUST_SRC_PATH = "${pkgs.rustPlatform.rustcSrc}/library";

            inputsFrom = [ inputs.self.packages.${system}.default ];

            packages = with pkgs; [
              bashInteractive

              pkgs'.rustc
              cargo
              pkgs'.clippy
              rustfmt
              rust-analyzer

              protobuf # for protoc/prost

              cargo-bloat
              cargo-edit
              cargo-udeps
              cargo-watch
              bacon

              age
            ];
          };
        }
      );
    };
}
