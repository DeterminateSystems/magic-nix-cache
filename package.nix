{ lib, stdenv, rustPlatform
, pkg-config
, installShellFiles
, nix
, boost
, darwin
}:

let
  ignoredPaths = [ ".github" "target" "book" ];

in rustPlatform.buildRustPackage rec {
  pname = "magic-nix-cache";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    filter = name: type: !(type == "directory" && builtins.elem (baseNameOf name) ignoredPaths);
    src = lib.cleanSource ./.;
  };

  nativeBuildInputs = [
    pkg-config
    installShellFiles
  ];

  buildInputs = [
    nix boost
  ] ++ lib.optionals stdenv.isDarwin (with darwin.apple_sdk.frameworks; [
    SystemConfiguration
  ]);

  cargoLock = {
    lockFile = ./Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  ATTIC_DISTRIBUTOR = "attic";

  # Recursive Nix is not stable yet
  doCheck = false;

  postFixup = ''
    rm -f $out/nix-support/propagated-build-inputs
  '';
}
