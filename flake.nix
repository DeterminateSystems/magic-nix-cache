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

        # We _can_ cross-compile to multiple systems with one shell :)
        #
        # Currently stdenv isn't set up to do that, but we can invoke
        # the setup mechinary in a sub-shell then compose the results.
        shellHook = let
          crossSystems = lib.filter (s: s != pkgs.system) (builtins.attrNames cranePkgs.crossPlatforms);
        in ''
          # Returns compiler environment variables for a platform
          #
          # getTargetFlags "suffixSalt" "nativeBuildInputs" "buildInputs"
          getTargetFlags() {
            # Here we only call the setup-hooks of nativeBuildInputs.
            #
            # What's off-limits for us:
            #
            # - findInputs
            # - activatePackage
            # - Other functions in stdenv setup that depend on the private accumulator variables
            (
              suffixSalt="$1"
              nativeBuildInputs="$2"
              buildInputs="$3"

              # Offsets for the nativeBuildInput (e.g., gcc)
              hostOffset=-1
              targetOffset=0

              # In stdenv, the hooks are first accumulated before being called.
              # Here we call them immediately
              addEnvHooks() {
                local depHostOffset="$1"
                # For simplicity, we only call the hook on buildInputs
                for pkg in $buildInputs; do
                  depTargetOffset=1
                  $2 $pkg
                done
              }

              unset _PATH
              unset NIX_CFLAGS_COMPILE
              unset NIX_LDFLAGS

              # For simplicity, we only call the setup-hooks of nativeBuildInputs
              for nbi in $nativeBuildInputs; do
                addToSearchPath _PATH "$nbi/bin"

                if [ -e "$nbi/nix-support/setup-hook" ]; then
                  source "$nbi/nix-support/setup-hook"
                fi
              done

              echo "export NIX_CFLAGS_COMPILE_''${suffixSalt}='$NIX_CFLAGS_COMPILE'"
              echo "export NIX_LDFLAGS_''${suffixSalt}='$NIX_LDFLAGS'"
              echo "export PATH=$PATH''${_PATH+:$_PATH}"
            )
          }

          target_flags=$(mktemp)
          ${lib.concatMapStrings (system: let
            crossPlatform = cranePkgs.crossPlatforms.${system};
          in ''
            getTargetFlags \
              "${crossPlatform.cc.suffixSalt}" \
              "${crossPlatform.cc} ${crossPlatform.cc.bintools}" \
              "${builtins.concatStringsSep " " (crossPlatform.buildInputs ++ crossPlatform.pkgs.stdenv.defaultBuildInputs)}" >$target_flags
            . $target_flags
          '') crossSystems}
          rm $target_flags

          # Suffix flags for current system as well
          export NIX_CFLAGS_COMPILE_${pkgs.stdenv.cc.suffixSalt}="$NIX_CFLAGS_COMPILE"
          export NIX_LDFLAGS_${pkgs.stdenv.cc.suffixSalt}="$NIX_LDFLAGS"
          unset NIX_CFLAGS_COMPILE
          unset NIX_LDFLAGS
        '';
      } // cranePkgs.cargoCrossEnvs);
      keygen = pkgs.mkShellNoCC {
        packages = with pkgs; [
          age
        ];
      };
    };
  });
}
