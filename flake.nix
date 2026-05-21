{
  description = "Static analyzer for NEF Nix expressions — finds catalog attribute-path references";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
    in {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          findRefs = pkgs.rustPlatform.buildRustPackage {
            pname = "find-refs";
            version = (builtins.fromTOML (builtins.readFile ./nef-catalog-refs/Cargo.toml)).package.version;
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "-p" "nef-catalog-refs" "--bin" "find-refs" ];
            cargoTestFlags = [ "-p" "nef-catalog-refs" ];
          };
        in {
          find-refs = findRefs;
          default = findRefs;
        });

      devShells = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [ rustc cargo clippy rustfmt ];
          };
        });
    };
}
