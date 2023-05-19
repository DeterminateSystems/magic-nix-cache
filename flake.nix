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
  in flake-utils.lib.eachSystem supportedSystems (system: let
    pkgs = import nixpkgs {
      inherit system;
      overlays = [
        rust-overlay.overlay
      ];
    };

    inherit (pkgs) lib;

    rustNightly = pkgs.rust-bin.nightly.${nightlyVersion}.default.override {
      extensions = [ "rust-src" "rust-analyzer-preview" ];
      targets = cranePkgs.cargoTargets;
    };

    cranePkgs = pkgs.callPackage ./crane.nix {
      inherit crane supportedSystems rustNightly;
    };
  in {
    packages = rec {
      inherit (cranePkgs) nix-actions-cache;
      default = nix-actions-cache;
    };
    devShells = {
      inputsFrom = [ cranePkgs.nix-actions-cache ];
      default = pkgs.mkShell ({
        packages = with pkgs; [
          bashInteractive
          rustNightly

          cargo-bloat
          cargo-edit
          cargo-udeps

          age
        ];
      } // cranePkgs.cargoCrossEnvs);
      keygen = pkgs.mkShellNoCC {
        packages = with pkgs; [
          age
        ];
      };
    };
  });
}
