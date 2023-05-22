{ stdenv
, pkgs
, lib
, crane
, rust
, rust-bin
, nix-gitignore
, supportedSystems
}:

let
  inherit (stdenv.hostPlatform) system;

  nightlyVersion = "2023-05-01";
  rustNightly = (pkgs.rust-bin.nightly.${nightlyVersion}.default.override {
    extensions = [ "rust-src" "rust-analyzer-preview" ];
    targets = cargoTargets;
  }).overrideAttrs (old: {
    # Remove the propagated libiconv since we want to add our static version
    depsTargetTargetPropagated = lib.filter (d: d.pname != "libiconv")
      (lib.flatten (old.depsTargetTargetPropagated or []));
  });

  # For easy cross-compilation in devShells
  # We are just composing the pkgsCross.*.stdenv.cc together
  crossPlatforms = let
    makeCrossPlatform = crossSystem: let
      pkgsCross =
        if crossSystem == system then pkgs
        else import pkgs.path {
          inherit system crossSystem;
          overlays = [];
        };

      rustTargetSpec = rust.toRustTargetSpec pkgsCross.pkgsStatic.stdenv.hostPlatform;
      rustTargetSpecUnderscored = builtins.replaceStrings [ "-" ] [ "_" ] rustTargetSpec;

      cargoLinkerEnv = lib.strings.toUpper "CARGO_TARGET_${rustTargetSpecUnderscored}_LINKER";
      cargoCcEnv = "CC_${rustTargetSpecUnderscored}"; # for ring

      cc = "${pkgsCross.stdenv.cc}/bin/${pkgsCross.stdenv.cc.targetPrefix}cc";
    in {
      name = crossSystem;
      value = {
        inherit rustTargetSpec cc;
        pkgs = pkgsCross;
        env = {
          "${cargoLinkerEnv}" = cc;
          "${cargoCcEnv}" = cc;
        };
      };
    };
    systems = lib.filter (s: s == system || lib.hasInfix "linux" s) supportedSystems;
  in builtins.listToAttrs (map makeCrossPlatform systems);

  cargoTargets = lib.mapAttrsToList (_: p: p.rustTargetSpec) crossPlatforms;
  cargoCrossEnvs = lib.foldl (acc: p: acc // p.env) {} (builtins.attrValues crossPlatforms);

  buildFor = system: let
    crossPlatform = crossPlatforms.${system};
    inherit (crossPlatform) pkgs;
    craneLib = (crane.mkLib pkgs).overrideToolchain rustNightly;
    crateName = craneLib.crateNameFromCargoToml {
      cargoToml = ./nix-actions-cache/Cargo.toml;
    };

    src = nix-gitignore.gitignoreSource [] ./.;

    commonArgs = {
      inherit (crateName) pname version;
      inherit src;

      buildInputs = with pkgs; []
      ++ lib.optionals pkgs.stdenv.isDarwin [
        darwin.apple_sdk.frameworks.Security
        (libiconv.override { enableStatic = true; enableShared = false; })
      ];

      cargoExtraArgs = "--target ${crossPlatform.rustTargetSpec}";

      cargoVendorDir = craneLib.vendorMultipleCargoDeps {
        inherit (craneLib.findCargoFiles src) cargoConfigs;
        cargoLockList = [
          ./Cargo.lock
          "${rustNightly.passthru.availableComponents.rust-src}/lib/rustlib/src/rust/Cargo.lock"
        ];
      };
    } // crossPlatform.env;

    crate = craneLib.buildPackage (commonArgs // {
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      # The resulting executable must be standalone
      allowedRequisites = [];
    });
  in crate;
in {
  inherit crossPlatforms cargoTargets cargoCrossEnvs rustNightly;

  nix-actions-cache = buildFor system;
}
