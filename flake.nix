{
  description = "A Nix-flake-based Rust development environment";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1.*.tar.gz";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    fenix.url = "github:nix-community/fenix";
  };

  outputs = { self, nixpkgs, rust-overlay, fenix }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forEachSupportedSystem = f:
        nixpkgs.lib.genAttrs supportedSystems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
              overlays = [
                rust-overlay.overlays.default
                (final: prev: {
                  rustToolchain =
                    fenix.packages.${system}.stable.toolchain;
                })
              ];
            };

            isLinux = pkgs.stdenv.isLinux;
            isDarwin = pkgs.stdenv.isDarwin;

            mkScript = name: text: pkgs.writeShellScriptBin name text;

            dynamicLinker =
              if isLinux then
                "${pkgs.glibc}/lib/ld-linux-x86-64.so.2"
              else
                "";

            buildScript = mkScript "build" ''
              echo "[build] Running cargo build..."
              cargo build "$@"
              if [[ "$*" == *"--release"* ]]; then
                profile="release"
              elif [[ "$*" == *"--profile="* ]]; then
                profile=$(echo "$*" | sed -n 's/.*--profile=\([^[:space:]]*\).*/\1/p')
              else
                profile="debug"
              fi
              binary_path="target/$profile/sage"
              if [[ -n "${dynamicLinker}" && -f "$binary_path" ]]; then
                echo "[build] Patching dynamic linker of ELF binary..."
                patchelf --set-interpreter ${dynamicLinker} "$binary_path"
              else
                echo "[build] Skipping patchelf (not Linux or binary missing)"
              fi
              if [[ -f "$binary_path" ]]; then
                cp "$binary_path" ./
                echo "[build] Copied $binary_path to ./"
              fi
              echo "[build] Done."
            '';

            releaseBinScript = mkScript "release-bin" ''
              echo "[release-bin] Compiling optimized release binary..."
              cargo build --release
              binary_path="target/release/sage"
              if [[ -n "${dynamicLinker}" && -f "$binary_path" ]]; then
                echo "[release-bin] Patching dynamic linker for portability..."
                patchelf --set-interpreter ${dynamicLinker} "$binary_path"
                echo "[release-bin] Portable release binary created at: $binary_path"
              else
                echo "[release-bin] Skipping patchelf (not Linux or binary missing)"
              fi
              if [[ -f "$binary_path" ]]; then
                cp "$binary_path" ./
                echo "[release-bin] Copied $binary_path to ./"
              fi
              echo "[release-bin] Done."
            '';

            lintScript = mkScript "lint" ''
              echo "[lint] Running cargo fmt..."
              cargo fmt -- --check
              echo "[lint] Running cargo clippy..."
              cargo clippy --all-targets --all-features -- -D warnings
              echo "[lint] Done."
            '';

            # Platform-specific packages
            extraPackages =
              if isLinux then [ pkgs.patchelf pkgs.mold ]
              else if isDarwin then [ pkgs.llvmPackages.libclang ]
              else [ ];
            extraEnv =
              if isLinux then { MOLD_PATH = "${pkgs.mold}/bin/mold"; }
              else { };
          in
          f {
            inherit pkgs;
            scripts = [ buildScript releaseBinScript lintScript ];
            extraPackages = extraPackages;
            extraEnv = extraEnv;
          }
        );
    in
    {
      devShells = forEachSupportedSystem (
        { pkgs, scripts, extraPackages, extraEnv }:
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustToolchain
              openssl
              pkg-config
              cargo-deny
              cargo-edit
              cargo-watch
              rust-analyzer
              clang
            ] ++ extraPackages ++ scripts;

            env = {
              RUST_SRC_PATH = "${pkgs.rustToolchain}/lib/rustlib/src/rust/library";
            } // extraEnv;
          };
        }
      );
    };
}