{
  description = "GitHub Actions-powered Nix binary cache";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-compat.follows = "flake-compat";
      inputs.flake-utils.follows = "flake-utils";
    };

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, ... }: let
    supportedSystems = flake-utils.lib.defaultSystems;
    nightlyVersion = "2023-05-01";

    makeCranePkgs = pkgs: let
      craneLib = crane.mkLib pkgs;
    in pkgs.callPackage ./crane.nix { inherit craneLib; };
  in flake-utils.lib.eachSystem supportedSystems (system: let
    pkgs = import nixpkgs {
      inherit system;
      overlays = [
        rust-overlay.overlay
      ];
    };

    inherit (pkgs) lib;

    crossPlatforms = let
      makeCrossPlatform = crossSystem: let
        pkgsCross = if crossSystem == system then pkgs else import nixpkgs {
          inherit system crossSystem;
          overlays = [];
        };
        rustTargetSpec = pkgs.rust.toRustTargetSpec pkgsCross.pkgsStatic.stdenv.hostPlatform;
        rustTargetSpecUnderscored = builtins.replaceStrings [ "-" ] [ "_" ] rustTargetSpec;
      in {
        inherit rustTargetSpec;
        cc = "${pkgsCross.stdenv.cc}/bin/${pkgsCross.stdenv.cc.targetPrefix}cc";
        cargoLinkerEnv = lib.strings.toUpper "CARGO_TARGET_${rustTargetSpecUnderscored}_LINKER";
        cargoCcEnv = "CC_${rustTargetSpecUnderscored}"; # for ring
      };
      systems = lib.filter (lib.hasInfix "linux") supportedSystems;
    in map makeCrossPlatform systems;

    rustNightly = pkgs.rust-bin.nightly.${nightlyVersion}.default.override {
      extensions = [ "rust-src" "rust-analyzer-preview" ];
      targets = map (p: p.rustTargetSpec) crossPlatforms;
    };

    cargoCrossEnvs = lib.listToAttrs (lib.flatten (map (p: [
      {
        name = p.cargoCcEnv;
        value = p.cc;
      }
      {
        name = p.cargoLinkerEnv;
        value = p.cc;
      }
    ]) crossPlatforms));
  in {
    devShells = {
      default = pkgs.mkShell ({
        packages = with pkgs; [
          bashInteractive
          rustNightly

          cargo-bloat
          cargo-edit
          cargo-udeps
        ]
        ++ lib.optional stdenv.hostPlatform.isDarwin darwin.apple_sdk.frameworks.Security
        ++ lib.optional stdenv.hostPlatform.isDarwin (libiconv.override { enableStatic = true; enableShared = false; })
        ;
      } // cargoCrossEnvs);
      keygen = pkgs.mkShellNoCC {
        packages = with pkgs; [
          age
        ];
      };
    };
  });
}
