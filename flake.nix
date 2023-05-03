{
  description = "Webb Relayer development environment";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # Rust
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        lib = pkgs.lib;
        toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShells.default = pkgs.mkShell {
          name = "relayer";
          nativeBuildInputs = [
            pkgs.pkg-config
            # Mold Linker for faster builds (only on Linux)
            (lib.optionals pkgs.stdenv.isLinux pkgs.mold)
          ];
          buildInputs = [
            # We want the unwrapped version, wrapped comes with nixpkgs' toolchain
            pkgs.rust-analyzer-unwrapped
            # Nodejs for test suite
            pkgs.nodejs_18
            pkgs.nodePackages.typescript-language-server
            pkgs.nodePackages.yarn
            # Finally the toolchain
            toolchain
          ];
          packages = [ ];
        };
        # Environment variables
        RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
      });
}