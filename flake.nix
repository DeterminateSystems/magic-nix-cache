{
  description = "GitHub Actions-powered Nix binary cache";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.2311.tar.gz";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane = {
      url = "https://flakehub.com/f/ipetkov/crane/0.16.3.tar.gz";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-compat.url = "https://flakehub.com/f/edolstra/flake-compat/1.0.1.tar.gz";

    nix.url = "https://flakehub.com/f/NixOS/nix/2.20.tar.gz";
  };

  outputs = { self, nixpkgs, nix, ... }@inputs:
    let
      overlays = [ inputs.rust-overlay.overlays.default nix.overlays.default ];
      supportedSystems = [
        "aarch64-linux"
        "x86_64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forEachSupportedSystem = f: nixpkgs.lib.genAttrs supportedSystems (system: f rec {
        pkgs = import nixpkgs { inherit overlays system; };
        cranePkgs = pkgs.callPackage ./crane.nix {
          inherit supportedSystems;
          inherit (inputs) crane;
          nix-flake = nix;
        };
        inherit (pkgs) lib;
      });
    in
    {
      packages = forEachSupportedSystem ({ pkgs, cranePkgs, ... }: rec {
        magic-nix-cache = pkgs.callPackage ./package.nix { };
        #inherit (cranePkgs) magic-nix-cache;
        default = magic-nix-cache;
      });

      devShells = forEachSupportedSystem ({ pkgs, cranePkgs, lib }: {
        default = pkgs.mkShell {
          inputsFrom = [ cranePkgs.magic-nix-cache ];
          packages = with pkgs; [
            bashInteractive
            cranePkgs.rustNightly

            cargo-bloat
            cargo-edit
            cargo-udeps
            bacon

            age
          ];
        };

        /*
        cross = pkgs.mkShell ({
          inputsFrom = [ cranePkgs.magic-nix-cache ];
          packages = with pkgs; [
            bashInteractive
            cranePkgs.rustNightly

            cargo-bloat
            cargo-edit
            cargo-udeps
            cargo-watch

            age
          ];
          shellHook =
            let
              crossSystems = lib.filter (s: s != pkgs.system) (builtins.attrNames cranePkgs.crossPlatforms);
            in
            ''
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
        */
      });
    };
}
