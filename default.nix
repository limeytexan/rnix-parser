{ pkgs ? import <nixpkgs> {} }:
pkgs.rustPlatform.buildRustPackage {
  pname = "find-refs";
  version = "0.1.0";
  src = pkgs.lib.cleanSource ./.;
  cargoLock.lockFile = ./Cargo.lock;
  cargoBuildFlags = [ "-p" "nef-catalog-refs" "--bin" "find-refs" ];
  cargoTestFlags = [ "-p" "nef-catalog-refs" ];
}
