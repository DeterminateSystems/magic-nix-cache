{
  description = "GitHub Actions-powered Nix binary cache";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.2311.tar.gz";

    fenix = {
      url = "https://flakehub.com/f/nix-community/fenix/0.1.1584.tar.gz";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-compat.url = "https://flakehub.com/f/edolstra/flake-compat/1.0.1.tar.gz";

    nix.url = "https://flakehub.com/f/NixOS/nix/~2.22.1.tar.gz";
  };

  outputs = { self, nixpkgs, fenix, naersk, nix, ... }@inputs:
    let
      supportedSystems = [
        "aarch64-linux"
        "x86_64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];

      forAllSystems = f: nixpkgs.lib.genAttrs supportedSystems (system: (forSystem system f));

      forSystem = system: f: f rec {
        inherit system;
        pkgs = import nixpkgs { inherit system; overlays = [ /* self.overlays.default */ nix.overlays.default ]; };
        lib = pkgs.lib;
      };

      fenixToolchain = system: with fenix.packages.${system};
        combine ([
          stable.clippy
          stable.rustc
          stable.cargo
          stable.rustfmt
          stable.rust-src
        ] ++ nixpkgs.lib.optionals (system == "x86_64-linux") [
          targets.x86_64-unknown-linux-musl.stable.rust-std
        ] ++ nixpkgs.lib.optionals (system == "aarch64-linux") [
          targets.aarch64-unknown-linux-musl.stable.rust-std
        ]);
    in
    {
      packages = forAllSystems ({ lib, system, pkgs, ... }: let
          toolchain = fenixToolchain pkgs.stdenv.system;
          naerskLib = pkgs.callPackage naersk {
            cargo = toolchain;
            rustc = toolchain;
          };
      in {
        magic-nix-cache = naerskLib.buildPackage {
          pname = "magic-nix-cache";
          version = (builtins.fromTOML (builtins.readFile ./magic-nix-cache/Cargo.toml)).package.version;
          src = builtins.path {
            name = "magic-nix-cache-source";
            path = self;
            filter = (path: type: baseNameOf path != "nix" && baseNameOf path != ".github");
          };

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.nix
              pkgs.boost # needed for clippy
            ]
            ++ lib.optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            (pkgs.libiconv.override { enableStatic = true; enableShared = false; })
          ];

          NIX_CFLAGS_LINK = lib.optionalString pkgs.stdenv.isDarwin "-lc++abi";
        };
        default = self.packages.${system}.magic-nix-cache;

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
            devShells = forAllSystems ({ lib, system, pkgs, ... }: let
        pkg = self.packages.${system}.default;
      in {
        default = pkgs.mkShell {
          inherit (pkg) buildInputs nativeBuildInputs NIX_CFLAGS_LINK;
      };
    });
    };
}
