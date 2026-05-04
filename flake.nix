{
  description = "GitHub Actions-powered Nix binary cache";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";

    crane.url = "https://flakehub.com/f/ipetkov/crane/*";

    fenix = {
      url = "https://flakehub.com/f/nix-community/fenix/0.1";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nix.url = "https://flakehub.com/f/DeterminateSystems/nix-src/=3.16.3";
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

      fenixToolchain =
        system:
        with inputs.fenix.packages.${system};
        combine (
          [
            stable.clippy
            stable.rustc
            stable.cargo
            stable.rustfmt
            stable.rust-src
          ]
          ++ inputs.nixpkgs.lib.optionals (system == "x86_64-linux") [
            targets.x86_64-unknown-linux-musl.stable.rust-std
          ]
          ++ inputs.nixpkgs.lib.optionals (system == "aarch64-linux") [
            targets.aarch64-unknown-linux-musl.stable.rust-std
          ]
        );

    in
    {

      packages = forEachSupportedSystem (
        { pkgs, system, ... }:
        let
          pkgs' = pkgs.pkgsStatic;
          toolchain = fenixToolchain system;
          craneLib = (inputs.crane.mkLib pkgs').overrideToolchain (_: toolchain);
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
          toolchain = fenixToolchain system;
          pkgs' = if pkgs.stdenv.isDarwin then pkgs else pkgs.pkgsStatic;
          rustTargetSpec = pkgs'.stdenv.hostPlatform.rust.rustcTargetSpec;
          rustTargetSpecEnv = pkgs'.lib.toUpper (builtins.replaceStrings [ "-" ] [ "_" ] rustTargetSpec);
        in
        {
          default = pkgs'.mkShell {
            env = {
              CARGO_BUILD_TARGET = rustTargetSpec;
              "CARGO_TARGET_${rustTargetSpecEnv}_LINKER" = "${pkgs'.stdenv.cc.targetPrefix}cc";
            };

            inputsFrom = [ inputs.self.packages.${system}.default ];

            depsBuildBuild = with pkgs; [
              buildPackages.stdenv.cc # for linking crates in the build environment
              lld
            ];

            packages =
              with pkgs;
              [
                toolchain

                bashInteractive

                cargo
                rustfmt
                rust-analyzer

                protobuf # for protoc/prost

                cargo-bloat
                cargo-edit
                cargo-udeps
                cargo-watch
                bacon

                age
              ]
              ++ (with pkgs'; [
                rustc
                clippy
              ]);
          };
        }
      );
    };
}
