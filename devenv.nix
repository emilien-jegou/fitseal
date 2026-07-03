{ pkgs, ... }:

{
  packages = [ pkgs.bacon ];

  languages.rust = {
    enable = true;
    channel = "nightly";
    components =[ "rustc" "cargo" "rust-src" "rustfmt" "rust-analyzer" "clippy" ];
    targets =[ "wasm32-unknown-unknown" "x86_64-unknown-linux-gnu" ];
  };

  dotenv.enable = true;
}
