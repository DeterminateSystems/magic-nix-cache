{ stdenv
, pkgs
, lib
, crane
, rustNightly
, rust
, nix-gitignore
, supportedSystems
}:

let
  inherit (stdenv.hostPlatform) system;

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

    pname = "nix-actions-cache";

    src = nix-gitignore.gitignoreSource [] ./.;

    buildInputs = with pkgs; []
      ++ lib.optionals pkgs.stdenv.isDarwin [
        darwin.apple_sdk.frameworks.Security
      ];

    # The Rust toolchain from rust-overlay has a dynamic libiconv in depsTargetTargetPropagated
    # Our static libiconv needs to take precedence
    nativeBuildInputs = with pkgs; []
      ++ lib.optionals pkgs.stdenv.isDarwin [
        (libiconv.override { enableStatic = true; enableShared = false; })
      ];

    cargoExtraArgs = "--target ${crossPlatform.rustTargetSpec}";

    cargoArtifacts = craneLib.buildDepsOnly ({
      inherit pname src buildInputs nativeBuildInputs cargoExtraArgs;

      doCheck = false;
    } // crossPlatform.env);
    crate = craneLib.buildPackage ({
      inherit pname src buildInputs nativeBuildInputs cargoExtraArgs;
      inherit cargoArtifacts;

      # The resulting executable must be standalone
      allowedRequisites = [];
    } // crossPlatform.env);
  in crate;
in {
  inherit crossPlatforms cargoTargets cargoCrossEnvs;

  nix-actions-cache = buildFor system;
}
