{ lib
, stdenv
, rustPlatform
, pkg-config
, installShellFiles
, nix
, boost
, darwin
, rust-analyzer
, clippy
, rustfmt
}:

let
  ignoredPaths = [ ".github" "target" "book" ];
  version = (builtins.fromTOML (builtins.readFile ./magic-nix-cache/Cargo.toml)).package.version;
in
rustPlatform.buildRustPackage rec {
  pname = "magic-nix-cache";
  inherit version;

  src = lib.cleanSourceWith {
    filter = name: type: !(type == "directory" && builtins.elem (baseNameOf name) ignoredPaths);
    src = lib.cleanSource ./.;
  };

  nativeBuildInputs = [
    pkg-config
    installShellFiles
    rust-analyzer
    clippy
    rustfmt
  ];

  buildInputs = [
    nix
    boost
  ] ++ lib.optionals stdenv.isDarwin (with darwin.apple_sdk.frameworks; [
    SystemConfiguration
  ]);

  cargoLock = {
    lockFile = ./Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  ATTIC_DISTRIBUTOR = "attic";

  # Hack to fix linking on macOS.
  NIX_CFLAGS_LINK = lib.optionalString stdenv.isDarwin "-lc++abi";

  # Recursive Nix is not stable yet
  doCheck = false;

  postFixup = ''
    rm -f $out/nix-support/propagated-build-inputs
  '';
}
