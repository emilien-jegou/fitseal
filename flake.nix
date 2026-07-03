{
  description = "A flake for fitseal";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }@inputs:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        buildToolchain = pkgs.rust-bin.nightly.latest.minimal.override {
          extensions = [ "rustc" "cargo" ];
        };

        rustPlatform = pkgs.makeRustPlatform {
          cargo = buildToolchain;
          rustc = buildToolchain;
        };

        commonBuildArgs = {
          src = pkgs.lib.cleanSource ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.makeWrapper
          ];
          doCheck = false;
        };

      in {
        packages = {
          fitseal = rustPlatform.buildRustPackage (commonBuildArgs // {
            pname = "fitseal";
            version = "0.0.1";
          });

          default = pkgs.symlinkJoin {
            name = "fitseal-workspace";
            paths = [
              self.packages.${system}.fitseal
            ];
          };
        };
      });
}
