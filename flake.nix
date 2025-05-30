{
  description = "GitHub Actions-powered Nix binary cache";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";

    crane.url = "https://flakehub.com/f/ipetkov/crane/*";

    nix.url = "https://flakehub.com/f/NixOS/nix/2";
  };

  outputs = inputs:
    let
      supportedSystems = [
        "aarch64-linux"
        "x86_64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];

      forEachSupportedSystem = f: inputs.nixpkgs.lib.genAttrs supportedSystems (system: f rec {
        pkgs = import inputs.nixpkgs {
          inherit system;
          overlays = [
            inputs.self.overlays.default
          ];
        };
        inherit system;
      });
    in
    {

      overlays.default = final: prev:
      let
          craneLib = inputs.crane.mkLib final;
          crateName = craneLib.crateNameFromCargoToml {
            cargoToml = ./magic-nix-cache/Cargo.toml;
          };

          commonArgs = {
            inherit (crateName) pname version;
            src = inputs.self;

            nativeBuildInputs = with final; [
              pkg-config
              protobuf
            ];

            buildInputs = [
              inputs.nix.packages.${final.stdenv.system}.default
              final.boost
            ];
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

      devShells = forEachSupportedSystem ({ system, pkgs }: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            clippy
            rustfmt
            rust-analyzer

            inputs.nix.packages.${stdenv.system}.default # for linking attic
            boost # for linking attic
            protobuf # for protoc/prost
            bashInteractive
            pkg-config

            cargo-bloat
            cargo-edit
            cargo-udeps
            cargo-watch
            bacon

            age
          ];

          RUST_SRC_PATH = "${pkgs.rustPlatform.rustcSrc}/library";
        };
      });
    };
}
