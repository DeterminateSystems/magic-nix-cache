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
  in flake-utils.lib.eachSystem supportedSystems (system: let
    pkgs = import nixpkgs {
      inherit system;
      overlays = [
        rust-overlay.overlay
      ];
    };

    inherit (pkgs) lib;

    cranePkgs = pkgs.callPackage ./crane.nix {
      inherit crane supportedSystems;
    };
  in {
    packages = rec {
      inherit (cranePkgs) nix-actions-cache;
      default = nix-actions-cache;
    };
    devShells = {
      default = pkgs.mkShell ({
        inputsFrom = [ cranePkgs.nix-actions-cache ];
        packages = with pkgs; [
          bashInteractive
          cranePkgs.rustNightly

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
